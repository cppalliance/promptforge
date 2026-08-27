// Push-to-talk voice capture over the /voice WebSocket: binary f32 PCM at
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

// Bundle-order-sensitive: voice.css overrides murm-ui composer rules at
// equal specificity, so it must land after the chat styles that main.ts
// imports first (esbuild emits CSS in module-graph order).
import "./voice.css";

import { DisposableStore, toDisposable, type IDisposable } from "../base/lifecycle";
import type { StatusBar } from "./status-bar";

export interface VoiceElements {
  mic: HTMLButtonElement;
  input: HTMLTextAreaElement;
}

/** The per-tab voice control; dispose() unwires the mic and discards a live take. */
export interface VoiceHandle extends IDisposable {
  discardIfRecording(): void;
}

/**
 * Asks the server whether transcription can run on the GPU. CPU whisper is
 * slow enough that the mic stays hidden instead. Any failure answers false:
 * a take that stalls for half a minute reads as broken, not as available.
 */
export async function voiceGpuAvailable(): Promise<boolean> {
  try {
    const response = await fetch("/voice/capability");
    if (!response.ok) {
      return false;
    }
    const body: unknown = await response.json();
    if (typeof body !== "object" || body === null || !("gpu" in body)) {
      return false;
    }
    return Reflect.get(body, "gpu") === true;
  } catch {
    return false;
  }
}

interface VoiceSession {
  ws: WebSocket;
  ctx: AudioContext;
  source: MediaStreamAudioSourceNode;
  node: AudioWorkletNode;
  stream: MediaStream;
}

interface TakeState {
  prefix: string;
  suffix: string;
}

// One socket's announced stream generation (services/protocol.ts
// StreamFrame), null until the server's announcement arrives. Tracked per
// socket because each take opens its own /voice connection and the server
// counts generations per connection.
interface StreamTracker {
  current: number | null;
}

export function setupVoice(elements: VoiceElements, statusBar: StatusBar): VoiceHandle {
  const { mic, input } = elements;
  let voice: VoiceSession | null = null;
  let suppressReplies = false;
  let take: TakeState | null = null;

  function setRecording(next: boolean): void {
    mic.classList.toggle("voice-mic--recording", next);
    mic.setAttribute("aria-pressed", String(next));
    mic.title = next ? "Stop recording" : "Push to talk";
  }

  // Programmatic value sets don't fire the textarea's "input" event, which
  // is what murm-ui's Input listens to for growing the composer and
  // re-enabling submit. Every voice-driven rewrite goes through it so the
  // canonical resizer runs; a local inline-height resizer would pin an
  // explicit height and disable the CSS field-sizing the app relies on.
  function notifyInput(): void {
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }

  function spliceValue(text: string): void {
    if (!take) return;
    input.value = take.prefix + text + take.suffix;
    const cursorPos = take.prefix.length + text.length;
    input.setSelectionRange(cursorPos, cursorPos);
    notifyInput();
  }

  // Tears down a session's audio half. The socket half is closed by the
  // caller, after any in-flight "stop" reply has had a chance to arrive.
  function releaseAudio(session: VoiceSession): void {
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
    input.value = take.prefix + finalText + take.suffix;
    const cursorPos = take.prefix.length + finalText.length;
    input.setSelectionRange(cursorPos, cursorPos);
    take = null;
    input.readOnly = false;
    input.classList.remove("mur-chat-input--recording");
    notifyInput();
  }

  function discardTake(): void {
    if (!take) return;
    input.value = take.prefix + take.suffix;
    const cursorPos = take.prefix.length;
    input.setSelectionRange(cursorPos, cursorPos);
    take = null;
    input.readOnly = false;
    input.classList.remove("mur-chat-input--recording");
    notifyInput();
  }

  // Handles one server text message. Returns true when the take is over and
  // the socket should close.
  function handleVoiceMessage(data: unknown, stream: StreamTracker): boolean {
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
    const start = input.selectionStart ?? input.value.length;
    const end = input.selectionEnd ?? input.value.length;
    const value = input.value;
    take = {
      prefix: value.slice(0, start),
      suffix: value.slice(end),
    };
    input.readOnly = true;
    input.classList.add("mur-chat-input--recording");
  }

  async function startVoice(): Promise<void> {
    if (!navigator.mediaDevices?.getUserMedia || !window.AudioContext || !window.WebSocket) {
      statusBar.showLocal("Voice capture is not available in this browser.", "error");
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
        `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/voice`,
      );
      ws.binaryType = "arraybuffer";
      await new Promise<void>((resolve, reject) => {
        ws!.addEventListener("open", () => resolve(), { once: true });
        ws!.addEventListener("error", () => reject(new Error("the /voice socket failed to open")), {
          once: true,
        });
      });
      // The context resamples the mic stream to 16 kHz before the worklet
      // sees it, so the wire format is 16 kHz mono f32 on any device.
      ctx = new AudioContext({ sampleRate: 16000 });
      await ctx.audioWorklet.addModule("/pcm-worklet.js");
      const source = ctx.createMediaStreamSource(stream);
      const node = new AudioWorkletNode(ctx, "pcm-capture");
      const session: VoiceSession = { ws, ctx, source, node, stream };
      suppressReplies = false;
      node.port.onmessage = (event) => {
        if (voice === session && ws!.readyState === WebSocket.OPEN) {
          ws!.send(event.data);
        }
      };
      const generation: StreamTracker = { current: null };
      ws.addEventListener("message", (event) => {
        if (handleVoiceMessage(event.data, generation)) {
          ws!.close();
        }
      });
      ws.addEventListener("close", () => {
        if (voice === session) {
          voice = null;
          setRecording(false);
          statusBar.setRecording(false);
          if (take) finishTake("");
          releaseAudio(session);
          statusBar.showLocal("The voice connection dropped.", "error");
        }
      });
      source.connect(node);
      // The worklet renders silence, so reaching the destination is safe and
      // keeps the graph pulling on every engine.
      node.connect(ctx.destination);
      voice = session;
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
      statusBar.showLocal(`Voice capture failed: ${(error as Error).message || error}`, "error");
    }
  }

  function stopVoice(): void {
    const session = voice;
    voice = null;
    setRecording(false);
    statusBar.setRecording(false);
    if (!session) {
      return;
    }
    releaseAudio(session);
    const { ws } = session;
    if (ws.readyState === WebSocket.OPEN) {
      ws.send("stop");
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

  function discardIfRecording(): void {
    const session = voice;
    if (!session) return;
    suppressReplies = true;
    voice = null;
    releaseAudio(session);
    session.ws.close();
    discardTake();
    setRecording(false);
    statusBar.setRecording(false);
  }

  const onMicClick = (): void => {
    if (voice) {
      stopVoice();
    } else {
      void startVoice();
    }
  };
  mic.addEventListener("click", onMicClick);

  const store = new DisposableStore();
  // Teardown order matters: the click listener detaches before the live
  // session is discarded, so a click cannot start a new take mid-teardown.
  store.add(toDisposable(() => mic.removeEventListener("click", onMicClick)));
  store.add(toDisposable(() => discardIfRecording()));

  return { discardIfRecording, dispose: (): void => store.dispose() };
}
