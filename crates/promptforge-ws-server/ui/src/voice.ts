// Push-to-talk voice capture over the /voice WebSocket: binary f32 PCM at
// 16 kHz mono in, "start"/"stop" control words, and JSON text frames out.
// Dictation behaves like typing at the cursor: each take captures the
// selection at record start, splices committed+tentative into that range,
// and sets readOnly so the user cannot disturb the insertion geometry.
// A `final` frame replaces the inserted region with polished text and
// releases readOnly; consecutive takes compose because the cursor position
// is captured fresh each time.

import type { StatusBar } from "./status-bar";

export interface VoiceElements {
  mic: HTMLButtonElement;
  status: HTMLDivElement;
  input: HTMLTextAreaElement;
}

export interface VoiceHandle {
  discardIfRecording(): void;
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

export function setupVoice(elements: VoiceElements, statusBar: StatusBar): VoiceHandle {
  const { mic, status, input } = elements;
  let voice: VoiceSession | null = null;
  let voiceStatusTimer = 0;
  let suppressReplies = false;
  let take: TakeState | null = null;

  function showVoiceStatus(text: string, isError: boolean): void {
    status.textContent = text;
    status.classList.toggle("voice-status--error", Boolean(isError));
    status.classList.add("voice-status--visible");
    clearTimeout(voiceStatusTimer);
    voiceStatusTimer = window.setTimeout(() => {
      status.classList.remove("voice-status--visible");
    }, 8000);
  }

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
  function handleVoiceMessage(data: unknown): boolean {
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
    } | null;
    try {
      msg = JSON.parse(data) as typeof msg;
    } catch {
      msg = null;
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
        showVoiceStatus("Transcript ready - edit, then send.", false);
      } else {
        finishTake("");
        const frames = typeof msg.frames === "number" ? msg.frames : 0;
        showVoiceStatus(`No speech detected (${frames} PCM frames captured).`, false);
      }
      return true;
    }
    // Anything else is shown verbatim and ends the take.
    finishTake("");
    showVoiceStatus(String(data), false);
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
      showVoiceStatus("Voice capture is not available in this browser.", true);
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
      showVoiceStatus(detail, true);
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
      ws.addEventListener("message", (event) => {
        if (handleVoiceMessage(event.data)) {
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
          showVoiceStatus("The voice connection dropped.", true);
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
      showVoiceStatus("Recording - press the mic button again to stop.", false);
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
      showVoiceStatus(`Voice capture failed: ${(error as Error).message || error}`, true);
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
      setTimeout(() => {
        if (ws.readyState === WebSocket.OPEN) {
          ws.close();
        }
      }, 120_000);
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
    showVoiceStatus("Recording discarded.", false);
  }

  mic.addEventListener("click", () => {
    if (voice) {
      stopVoice();
    } else {
      void startVoice();
    }
  });

  return { discardIfRecording };
}
