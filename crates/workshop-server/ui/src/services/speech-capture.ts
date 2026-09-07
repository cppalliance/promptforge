import { Emitter, type Event } from "../base/event";
import { Disposable } from "../base/lifecycle";

const OUTPUT_SAMPLE_RATE = 24_000;
const FLUSH_TIMEOUT_MS = 1_000;

/** A successful microphone lifecycle operation. */
export type SpeechCaptureSuccess =
  | { readonly ok: true; readonly kind: "started" }
  | { readonly ok: true; readonly kind: "stopped" }
  | { readonly ok: true; readonly kind: "cleared" };

/** A microphone failure that leaves capture available for another attempt. */
export type SpeechCaptureFailure = {
  readonly ok: false;
  readonly kind:
    | "permission-denied"
    | "device-unavailable"
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

function errorText(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "object" && error !== null) {
    const message = Reflect.get(error, "message");
    if (typeof message === "string") {
      return message;
    }
  }
  return String(error);
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
 */
export class SpeechCaptureService extends Disposable {
  private readonly audio = this._register(new Emitter<ArrayBuffer>());
  private session: SpeechCaptureSession | null = null;
  private phase: "idle" | "starting" | "recording" | "stopping" = "idle";
  private disposed = false;

  /** Fires for each owned little-endian mono PCM16 block at 24 kHz. */
  readonly onAudio: Event<ArrayBuffer> = this.audio.event;

  constructor(private readonly backend: SpeechCaptureBackend = browserBackend()) {
    super();
  }

  /** Whether a microphone graph is currently recording. */
  get recording(): boolean {
    return this.phase === "recording";
  }

  /** Opens capture, returning a recoverable outcome instead of throwing. */
  async start(): Promise<SpeechCaptureOutcome> {
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
      return { ok: true, kind: "started" };
    } catch (error) {
      this.phase = "idle";
      return startFailure(error);
    }
  }

  /** Flushes and closes capture, returning any close failure as recoverable. */
  async stop(): Promise<SpeechCaptureOutcome> {
    const session = this.session;
    if (session === null) {
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
    }
  }

  /** Drops carried worklet audio while leaving an active microphone open. */
  clear(): SpeechCaptureOutcome {
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
    super.dispose();
  }
}
