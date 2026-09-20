import { Emitter, type Event } from "../base/event";
import { Disposable } from "../base/lifecycle";
import { errorText } from "./error-catalog";
import { createServiceToken, type ServiceToken } from "./service-registry";

const OUTPUT_SAMPLE_RATE = 24_000;
const FLUSH_TIMEOUT_MS = 1_000;

/** A successful microphone lifecycle operation. */
export type SpeechCaptureSuccess =
  | { readonly ok: true; readonly kind: "started" }
  | { readonly ok: true; readonly kind: "stopped" }
  | { readonly ok: true; readonly kind: "cleared" };

/**
 * A microphone failure that leaves capture available for another attempt.
 * `busy` means another owner token holds the microphone right now; the
 * caller's own retry succeeds once that owner's take ends.
 */
export type SpeechCaptureFailure = {
  readonly ok: false;
  readonly kind:
    | "permission-denied"
    | "device-unavailable"
    | "busy"
    | "start-failed"
    | "stop-failed"
    | "clear-failed";
  readonly message: string;
  readonly recoverable: true;
};

/** The result of a capture lifecycle operation. */
export type SpeechCaptureOutcome = SpeechCaptureSuccess | SpeechCaptureFailure;

/** One opened microphone graph owned by a capture service. */
export interface SpeechCaptureSession {
  /** Drops buffered audio without stopping capture. */
  clear(): void;
  /** Flushes buffered audio, then stops the graph. */
  stop(): Promise<void>;
  /** Immediately releases every graph resource. */
  dispose(): void;
}

/** Injectable browser-audio boundary used by the DOM-free capture service. */
export interface SpeechCaptureBackend {
  /** Opens a 24 kHz mono PCM16 capture graph. */
  open(emitAudio: (chunk: ArrayBuffer) => void): Promise<SpeechCaptureSession>;
}

type OpenFailureKind = "permission" | "device" | "start";

interface OpenFailure {
  readonly kind: OpenFailureKind;
  readonly message: string;
}

function openFailure(kind: OpenFailureKind, error: unknown): OpenFailure {
  return { kind, message: errorText(error) };
}

function classifyMediaFailure(error: unknown): OpenFailure {
  const name =
    typeof error === "object" && error !== null && typeof Reflect.get(error, "name") === "string"
      ? (Reflect.get(error, "name") as string)
      : "";
  if (name === "NotAllowedError" || name === "SecurityError") {
    return openFailure("permission", error);
  }
  return openFailure("device", error);
}

class BrowserSpeechCaptureSession implements SpeechCaptureSession {
  private disposed = false;
  private flush:
    | {
        readonly resolve: () => void;
        readonly reject: (error: Error) => void;
        readonly timer: ReturnType<typeof setTimeout>;
      }
    | null = null;

  constructor(
    private readonly context: AudioContext,
    private readonly stream: MediaStream,
    private readonly source: MediaStreamAudioSourceNode,
    private readonly node: AudioWorkletNode,
    emitAudio: (chunk: ArrayBuffer) => void,
  ) {
    this.node.port.onmessage = (event: MessageEvent<unknown>) => {
      if (event.data instanceof ArrayBuffer) {
        emitAudio(event.data);
      } else if (
        typeof event.data === "object" &&
        event.data !== null &&
        Reflect.get(event.data, "type") === "flushed"
      ) {
        this.finishFlush();
      }
    };
  }

  clear(): void {
    if (!this.disposed) {
      this.node.port.postMessage({ type: "clear" });
    }
  }

  async stop(): Promise<void> {
    if (this.disposed) {
      return;
    }
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.flush = null;
        reject(new Error("audio worklet flush timed out"));
      }, FLUSH_TIMEOUT_MS);
      this.flush = { resolve, reject, timer };
      this.node.port.postMessage({ type: "flush" });
    });
    this.releaseGraph();
    await this.context.close();
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.cancelFlush();
    this.releaseGraph();
    // dispose() is synchronous, so context shutdown completes in the background.
    void this.context.close().catch(() => {});
  }

  private finishFlush(): void {
    const flush = this.flush;
    if (flush === null) {
      return;
    }
    this.flush = null;
    clearTimeout(flush.timer);
    flush.resolve();
  }

  private cancelFlush(): void {
    const flush = this.flush;
    if (flush === null) {
      return;
    }
    this.flush = null;
    clearTimeout(flush.timer);
    flush.reject(new Error("speech capture was disposed while flushing"));
  }

  private releaseGraph(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.node.port.onmessage = null;
    this.source.disconnect();
    this.node.disconnect();
    for (const track of this.stream.getTracks()) {
      track.stop();
    }
  }
}

function browserBackend(): SpeechCaptureBackend {
  return {
    async open(emitAudio): Promise<SpeechCaptureSession> {
      if (
        typeof navigator === "undefined" ||
        !navigator.mediaDevices?.getUserMedia ||
        typeof AudioContext === "undefined" ||
        typeof AudioWorkletNode === "undefined"
      ) {
        throw openFailure("device", new Error("microphone capture is unavailable"));
      }

      let stream: MediaStream;
      try {
        stream = await navigator.mediaDevices.getUserMedia({
          audio: {
            channelCount: 1,
            sampleRate: OUTPUT_SAMPLE_RATE,
            echoCancellation: true,
            noiseSuppression: true,
          },
        });
      } catch (error) {
        throw classifyMediaFailure(error);
      }

      let context: AudioContext | null = null;
      let source: MediaStreamAudioSourceNode | null = null;
      let node: AudioWorkletNode | null = null;
      try {
        context = new AudioContext({ sampleRate: OUTPUT_SAMPLE_RATE });
        if (context.sampleRate !== OUTPUT_SAMPLE_RATE) {
          throw new Error(
            `browser opened audio at ${context.sampleRate} Hz instead of ${OUTPUT_SAMPLE_RATE} Hz`,
          );
        }
        await context.audioWorklet.addModule("/pcm-worklet.js");
        source = context.createMediaStreamSource(stream);
        node = new AudioWorkletNode(context, "pcm16-capture");
        const session = new BrowserSpeechCaptureSession(
          context,
          stream,
          source,
          node,
          emitAudio,
        );
        source.connect(node);
        node.connect(context.destination);
        await context.resume();
        return session;
      } catch (error) {
        node?.disconnect();
        source?.disconnect();
        for (const track of stream.getTracks()) {
          track.stop();
        }
        if (context !== null) {
          // Preserve the graph-start error even when best-effort cleanup also fails.
          await context.close().catch(() => {});
        }
        throw openFailure("start", error);
      }
    },
  };
}

function failure(kind: SpeechCaptureFailure["kind"], error: unknown): SpeechCaptureFailure {
  return { ok: false, kind, message: errorText(error), recoverable: true };
}

function startFailure(error: unknown): SpeechCaptureFailure {
  const kind =
    typeof error === "object" && error !== null ? Reflect.get(error, "kind") : undefined;
  if (kind === "permission") {
    return failure("permission-denied", error);
  }
  if (kind === "device") {
    return failure("device-unavailable", error);
  }
  return failure("start-failed", error);
}

/**
 * Owns browser microphone capture without touching the DOM. Audio and every
 * lifecycle failure are values so a view can recover without rebuilding it.
 *
 * One service is shared by every dictation surface in a window, so the
 * microphone has an owner: the opaque token handed to the `start()` that
 * opened it. Only that token can stop or clear the take; any other token's
 * start is refused with `busy`, and its stop and clear are no-op successes.
 * Ownership is held from a successful start through the end of its stop
 * (the flush still belongs to the owner), then released.
 */
export class SpeechCaptureService extends Disposable {
  private readonly audio = this._register(new Emitter<ArrayBuffer>());
  private readonly ownerChange = this._register(new Emitter<symbol | null>());
  private session: SpeechCaptureSession | null = null;
  private phase: "idle" | "starting" | "recording" | "stopping" = "idle";
  private currentOwner: symbol | null = null;
  private disposed = false;

  /** Fires for each owned little-endian mono PCM16 block at 24 kHz. */
  readonly onAudio: Event<ArrayBuffer> = this.audio.event;

  /** Fires with the new owner when the microphone is taken, `null` when released. */
  readonly onOwnerChange: Event<symbol | null> = this.ownerChange.event;

  constructor(private readonly backend: SpeechCaptureBackend = browserBackend()) {
    super();
  }

  /** Whether a microphone graph is currently recording. */
  get recording(): boolean {
    return this.phase === "recording";
  }

  /** The token whose start opened the live take, or `null` when free. */
  get owner(): symbol | null {
    return this.currentOwner;
  }

  /**
   * Opens capture for `owner`, returning a recoverable outcome instead of
   * throwing. `busy` when another owner holds the microphone; the existing
   * `start-failed` for a same-owner double start or a start while the
   * graph is still opening or closing, whoever asks: the owner keeps the
   * flush, but the closing window is a transient, not another window's take.
   */
  async start(owner: symbol): Promise<SpeechCaptureOutcome> {
    if (this.phase === "recording" && this.currentOwner !== owner) {
      return failure("busy", new Error("speech capture is held by another owner"));
    }
    if (this.disposed || this.phase !== "idle") {
      return failure("start-failed", new Error("speech capture is already active"));
    }
    this.phase = "starting";
    try {
      const session = await this.backend.open((chunk) => this.audio.fire(chunk));
      if (this.disposed) {
        session.dispose();
        return failure("start-failed", new Error("speech capture was disposed while starting"));
      }
      this.session = session;
      this.phase = "recording";
      this.setOwner(owner);
      return { ok: true, kind: "started" };
    } catch (error) {
      this.phase = "idle";
      return startFailure(error);
    }
  }

  /**
   * Flushes and closes the owner's capture, returning any close failure as
   * recoverable. A non-owner's stop is a no-op success: the take runs on.
   */
  async stop(owner: symbol): Promise<SpeechCaptureOutcome> {
    const session = this.session;
    if (session === null || this.currentOwner !== owner) {
      return { ok: true, kind: "stopped" };
    }
    this.phase = "stopping";
    try {
      await session.stop();
      return { ok: true, kind: "stopped" };
    } catch (error) {
      return failure("stop-failed", error);
    } finally {
      session.dispose();
      if (this.session === session) {
        this.session = null;
      }
      this.phase = "idle";
      this.setOwner(null);
    }
  }

  /**
   * Drops carried worklet audio while leaving the owner's microphone open.
   * A non-owner's clear is a no-op success.
   */
  clear(owner: symbol): SpeechCaptureOutcome {
    if (this.currentOwner !== owner) {
      return { ok: true, kind: "cleared" };
    }
    try {
      this.session?.clear();
      return { ok: true, kind: "cleared" };
    } catch (error) {
      return failure("clear-failed", error);
    }
  }

  override dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.session?.dispose();
    this.session = null;
    this.phase = "idle";
    this.setOwner(null);
    super.dispose();
  }

  private setOwner(owner: symbol | null): void {
    if (this.currentOwner === owner) {
      return;
    }
    this.currentOwner = owner;
    this.ownerChange.fire(owner);
  }
}

/**
 * The registry token for the composition root's SpeechCaptureService.
 * The agent session's dictation resolves it through the service registry
 * instead of the dock's createComponent seam. Registered by the
 * composition root at boot; unregistered in tests that drive panels
 * standalone.
 */
export const SPEECH_CAPTURE: ServiceToken<SpeechCaptureService> =
  createServiceToken<SpeechCaptureService>("workshop.speechCapture");
