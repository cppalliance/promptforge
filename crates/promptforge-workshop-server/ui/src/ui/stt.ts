// Push-to-talk dictation over the /stt WebSocket: binary f32 PCM at
// 16 kHz mono in, "start"/"stop" control words, and JSON text frames out.
// The server answers each "start" with a `stream` frame announcing the
// take's generation and tags every interim/final frame with it; frames
// from an older generation are stale (a stop/restart race) and dropped.
// Dictation behaves like typing at the cursor: each take captures the
// selection at record start, splices committed+tentative into that range,
// and sets readOnly so the user cannot disturb the insertion geometry.
// A `final` frame replaces the inserted region with polished text and
// releases readOnly; consecutive takes compose because the cursor position
// is captured fresh each time.

import "./stt.css";

import { DisposableStore, toDisposable, type IDisposable } from "../base/lifecycle";

/**
 * What dictation needs from its host input: a text target the take can
 * splice the transcript into. Offsets are the target's own text
 * coordinates - a textarea's string offsets, the prompt editor's
 * ProseMirror positions. A take only ever combines a captured `start`
 * with the length of the text it last inserted there, which is valid in
 * both spaces.
 */
export interface SttInputTarget {
  /** The current selection: the take's insertion anchor. */
  getSelection(): { start: number; end: number };
  /** Replaces [from, to] with text, leaving the cursor after the inserted text. */
  replaceRange(from: number, to: number, text: string): void;
  /** Locks the input against typing while a take splices, or releases it. */
  setReadOnly(readOnly: boolean): void;
  /** Returns focus to the input; a landed final calls it. */
  focus(): void;
}

export interface SttElements {
  mic: HTMLButtonElement;
  input: SttInputTarget;
}

/**
 * The textarea target: one-line wrappers over the native selection API,
 * behavior unchanged from when setupStt held the textarea directly.
 */
export function textareaSttTarget(input: HTMLTextAreaElement): SttInputTarget {
  return {
    getSelection: () => ({
      start: input.selectionStart ?? input.value.length,
      end: input.selectionEnd ?? input.value.length,
    }),
    replaceRange: (from, to, text) => {
      input.setRangeText(text, from, to, "end");
      // Programmatic value sets don't fire the textarea's "input" event,
      // so every dictation-driven rewrite dispatches it: dictation
      // behaves like typing to whatever listens on the input.
      input.dispatchEvent(new Event("input", { bubbles: true }));
    },
    setReadOnly: (readOnly) => {
      input.readOnly = readOnly;
      input.classList.toggle("stt-input--recording", readOnly);
    },
    focus: () => input.focus(),
  };
}

/**
 * The status-bar slice dictation paints: local messages (blockers, capture
 * failures, an empty take) and the REC badge. `StatusBar` satisfies it
 * structurally; tests hand in a recording fake.
 */
export interface SttStatus {
  showLocal(label: string, severity: "info" | "error"): void;
  setRecording(on: boolean): void;
}

/**
 * What blocks starting a take right now, as a user-readable reason, or
 * null when a take may start. Consulted on every mic click: the mic stays
 * visible and clickable even when blocked, so the click can name the
 * blocker on the status bar instead of the control silently disappearing.
 */
export type SttBlocker = () => string | null;

/** The per-tab dictation control; dispose() unwires the mic and discards a live take. */
export interface SttHandle extends IDisposable {
  discardIfRecording(): void;
}

/** The server's STT capability answer: what dictation can do here. */
export interface SttCapability {
  /** Whether transcription can run on the GPU. */
  gpu: boolean;
  /** Whether an STT engine is provisioned and loaded in the active profile. */
  engine: boolean;
}

/**
 * Asks the server what dictation can do here. Any failure - transport, status,
 * or a malformed body - answers null, which the caller treats as blocked.
 */
export async function sttCapability(): Promise<SttCapability | null> {
  try {
    const response = await fetch("/stt/capability");
    if (!response.ok) {
      return null;
    }
    const body: unknown = await response.json();
    if (typeof body !== "object" || body === null) {
      return null;
    }
    const gpu = Reflect.get(body, "gpu");
    const engine = Reflect.get(body, "engine");
    if (typeof gpu !== "boolean" || typeof engine !== "boolean") {
      return null;
    }
    return { gpu, engine };
  } catch {
    return null;
  }
}

interface SttSession {
  ws: WebSocket;
  ctx: AudioContext;
  source: MediaStreamAudioSourceNode;
  node: AudioWorkletNode;
  stream: MediaStream;
}

interface TakeState {
  /** The offset where the take's inserted region starts. */
  from: number;
  /**
   * The length of the region the take owns: the selection it captured at
   * record start, then the last splice it wrote.
   */
  length: number;
}

// One socket's announced stream generation (services/protocol.ts
// StreamFrame), null until the server's announcement arrives. Tracked per
// socket because each take opens its own /stt connection and the server
// counts generations per connection.
interface StreamTracker {
  current: number | null;
}

export function setupStt(
  elements: SttElements,
  statusBar: SttStatus,
  blocked: SttBlocker,
): SttHandle {
  const { mic, input } = elements;
  let active: SttSession | null = null;
  let suppressReplies = false;
  let take: TakeState | null = null;
  // A stopped take's socket while its final is still in flight. The take
  // (and the input's readOnly) stays open until that final lands, the
  // socket drops, or a discard closes it; without this handle a discard
  // in the stop window would see no session and leave the input locked.
  let pendingFinal: WebSocket | null = null;

  function setRecording(next: boolean): void {
    mic.classList.toggle("stt-mic--recording", next);
    mic.setAttribute("aria-pressed", String(next));
    mic.title = next ? "Stop recording" : "Push to talk";
  }

  function spliceValue(text: string): void {
    if (!take) return;
    input.replaceRange(take.from, take.from + take.length, text);
    take.length = text.length;
  }

  // Tears down a session's audio half. The socket half is closed by the
  // caller, after any in-flight "stop" reply has had a chance to arrive.
  function releaseAudio(session: SttSession): void {
    session.node.port.onmessage = null;
    session.source.disconnect();
    session.node.disconnect();
    for (const track of session.stream.getTracks()) {
      track.stop();
    }
    // Best effort: a failed close leaves nothing the page can still act on.
    session.ctx.close().catch(() => {});
  }

  function finishTake(finalText: string): void {
    if (!take) return;
    spliceValue(finalText);
    take = null;
    input.setReadOnly(false);
  }

  function discardTake(): void {
    if (!take) return;
    spliceValue("");
    take = null;
    input.setReadOnly(false);
  }

  // Handles one server text message. Returns true when the take is over and
  // the socket should close.
  function handleSttMessage(data: unknown, stream: StreamTracker): boolean {
    if (suppressReplies) return true;
    if (typeof data !== "string") {
      return true;
    }
    let msg: {
      type?: unknown;
      text?: unknown;
      committed?: unknown;
      tentative?: unknown;
      frames?: unknown;
      generation?: unknown;
    } | null;
    try {
      msg = JSON.parse(data) as typeof msg;
    } catch {
      msg = null;
    }
    if (msg && msg.type === "stream") {
      stream.current = typeof msg.generation === "number" ? msg.generation : null;
      return false;
    }
    // A frame tagged with a generation other than the announced one belongs
    // to a take the server has already superseded (a stop/restart race):
    // drop it and keep listening for the current generation. A frame with
    // no generation, or one arriving before any announcement, is treated
    // as current, so the client tolerates a server that never announces.
    if (
      msg &&
      (msg.type === "interim" || msg.type === "final") &&
      typeof msg.generation === "number" &&
      stream.current !== null &&
      msg.generation !== stream.current
    ) {
      return false;
    }
    if (msg && msg.type === "interim") {
      const committed = typeof msg.committed === "string" ? msg.committed : "";
      const tentative = typeof msg.tentative === "string" ? msg.tentative : "";
      const gap = committed !== "" && tentative !== "" && !/\s$/.test(committed) ? " " : "";
      spliceValue(committed + gap + tentative);
      return false;
    }
    if (msg && msg.type === "final") {
      const raw = typeof msg.text === "string" ? msg.text : "";
      const text = raw.trimEnd();
      if (text !== "") {
        finishTake(text);
        input.focus();
      } else {
        finishTake("");
        const frames = typeof msg.frames === "number" ? msg.frames : 0;
        statusBar.showLocal(`No speech detected (${frames} PCM frames captured).`, "info");
      }
      return true;
    }
    // Anything else is shown verbatim and ends the take.
    finishTake("");
    statusBar.showLocal(String(data), "error");
    return true;
  }

  function beginTake(): void {
    const { start, end } = input.getSelection();
    take = { from: start, length: end - start };
    input.setReadOnly(true);
  }

  async function startStt(): Promise<void> {
    if (!navigator.mediaDevices?.getUserMedia || !window.AudioContext || !window.WebSocket) {
      statusBar.showLocal("Dictation is not available in this browser.", "error");
      return;
    }
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          channelCount: 1,
          sampleRate: 16000,
          echoCancellation: true,
          noiseSuppression: true,
        },
      });
    } catch (error) {
      const detail =
        error instanceof Error && error.name === "NotAllowedError"
          ? "microphone permission denied"
          : `microphone unavailable: ${(error as Error).message || error}`;
      statusBar.showLocal(detail, "error");
      return;
    }
    let ws: WebSocket | undefined;
    let ctx: AudioContext | undefined;
    try {
      ws = new WebSocket(
        `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/stt`,
      );
      ws.binaryType = "arraybuffer";
      await new Promise<void>((resolve, reject) => {
        ws!.addEventListener("open", () => resolve(), { once: true });
        ws!.addEventListener("error", () => reject(new Error("the /stt socket failed to open")), {
          once: true,
        });
      });
      // The context resamples the mic stream to 16 kHz before the worklet
      // sees it, so the wire format is 16 kHz mono f32 on any device.
      ctx = new AudioContext({ sampleRate: 16000 });
      await ctx.audioWorklet.addModule("/pcm-worklet.js");
      const source = ctx.createMediaStreamSource(stream);
      const node = new AudioWorkletNode(ctx, "pcm-capture");
      const session: SttSession = { ws, ctx, source, node, stream };
      suppressReplies = false;
      node.port.onmessage = (event) => {
        if (active === session && ws!.readyState === WebSocket.OPEN) {
          ws!.send(event.data);
        }
      };
      const generation: StreamTracker = { current: null };
      ws.addEventListener("message", (event) => {
        if (handleSttMessage(event.data, generation)) {
          if (pendingFinal === ws) {
            pendingFinal = null;
          }
          ws!.close();
        }
      });
      ws.addEventListener("close", () => {
        if (active === session) {
          active = null;
          setRecording(false);
          statusBar.setRecording(false);
          if (take) finishTake("");
          releaseAudio(session);
          statusBar.showLocal("The dictation connection dropped.", "error");
        } else if (pendingFinal === ws) {
          // Dropped, or the stop deadline closed it, before the final
          // landed: the take ends as a live drop does, on the pre-take text.
          pendingFinal = null;
          finishTake("");
          statusBar.showLocal("The dictation connection dropped before the final transcript.", "error");
        }
      });
      source.connect(node);
      // The worklet renders silence, so reaching the destination is safe and
      // keeps the graph pulling on every engine.
      node.connect(ctx.destination);
      active = session;
      beginTake();
      ws.send("start");
      setRecording(true);
      statusBar.setRecording(true);
    } catch (error) {
      for (const track of stream.getTracks()) {
        track.stop();
      }
      if (ws) {
        ws.close();
      }
      if (ctx) {
        ctx.close().catch(() => {});
      }
      statusBar.showLocal(`Dictation failed: ${(error as Error).message || error}`, "error");
    }
  }

  function stopStt(): void {
    const session = active;
    active = null;
    setRecording(false);
    statusBar.setRecording(false);
    if (!session) {
      return;
    }
    releaseAudio(session);
    const { ws } = session;
    if (ws.readyState === WebSocket.OPEN) {
      ws.send("stop");
      pendingFinal = ws;
      // The final whisper pass can take 30+ seconds on CPU; give it time.
      // The message listener closes the socket when the final reply arrives.
      const deadline = setTimeout(() => {
        if (ws.readyState === WebSocket.OPEN) {
          ws.close();
        }
      }, 120_000);
      // The post-stop socket deliberately outlives the session so the
      // final reply can land, but the handle still owns it: disposing the
      // tab closes the socket and cancels the deadline instead of leaving
      // both live (and splicing into a dead textarea) for two minutes.
      store.add(
        toDisposable(() => {
          clearTimeout(deadline);
          if (ws.readyState === WebSocket.OPEN) {
            ws.close();
          }
        }),
      );
    }
  }

  // Ends a take that is still open: recording, or stopped with its final
  // in flight. Either way the socket closes, a late reply is ignored, and
  // the input returns to its pre-take text with readOnly lifted.
  function discardIfRecording(): void {
    const session = active;
    const awaited = pendingFinal;
    if (!session && !awaited) return;
    suppressReplies = true;
    active = null;
    pendingFinal = null;
    if (session) {
      releaseAudio(session);
      session.ws.close();
    }
    // A new take may have started while the previous stop's final was
    // still in flight; both sockets go.
    if (awaited) {
      awaited.close();
    }
    discardTake();
    setRecording(false);
    statusBar.setRecording(false);
  }

  const onMicClick = (): void => {
    if (active) {
      stopStt();
      return;
    }
    const reason = blocked();
    if (reason !== null) {
      statusBar.showLocal(reason, "info");
      return;
    }
    void startStt();
  };
  mic.addEventListener("click", onMicClick);

  const store = new DisposableStore();
  // Teardown order matters: the click listener detaches before the live
  // session is discarded, so a click cannot start a new take mid-teardown.
  store.add(toDisposable(() => mic.removeEventListener("click", onMicClick)));
  store.add(toDisposable(() => discardIfRecording()));

  return { discardIfRecording, dispose: (): void => store.dispose() };
}
