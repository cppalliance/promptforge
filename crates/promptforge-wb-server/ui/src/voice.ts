// Push-to-talk voice capture over the /voice WebSocket. The wire protocol is
// unchanged from the pre-migration UI: binary f32 PCM at 16 kHz mono in,
// "start"/"stop" control words, and JSON text frames out (`interim` updates
// the textarea live, `final` replaces it with the polished transcript).

import type { StatusBar } from "./status-bar";

const MAX_TEXTAREA_HEIGHT_VH = 40;

export interface VoiceElements {
  mic: HTMLButtonElement;
  status: HTMLDivElement;
  input: HTMLTextAreaElement;
}

interface VoiceSession {
  ws: WebSocket;
  ctx: AudioContext;
  source: MediaStreamAudioSourceNode;
  node: AudioWorkletNode;
  stream: MediaStream;
}

export function setupVoice(elements: VoiceElements, statusBar: StatusBar): void {
  const { mic, status, input } = elements;
  let voice: VoiceSession | null = null;
  let voiceStatusTimer = 0;

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

  function resizeInput(): void {
    const maxPx = window.innerHeight * (MAX_TEXTAREA_HEIGHT_VH / 100);
    input.style.height = "auto";
    input.style.height = Math.min(input.scrollHeight, maxPx) + "px";
  }

  function showInterim(text: string): void {
    input.value = text;
    resizeInput();
  }

  function clearInterim(): void {
    input.value = "";
    resizeInput();
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

  // Handles one server text message. Returns true when the take is over and
  // the socket should close.
  function handleVoiceMessage(data: unknown): boolean {
    if (typeof data !== "string") {
      return true;
    }
    let msg: { type?: unknown; text?: unknown; frames?: unknown } | null;
    try {
      msg = JSON.parse(data) as typeof msg;
    } catch {
      msg = null;
    }
    if (msg && msg.type === "interim" && typeof msg.text === "string") {
      showInterim(msg.text);
      return false;
    }
    if (msg && msg.type === "final") {
      clearInterim();
      const text = typeof msg.text === "string" ? msg.text.trim() : "";
      if (text !== "") {
        input.value = text;
        // murm-ui's Input listens for "input" to re-enable submit.
        input.dispatchEvent(new Event("input", { bubbles: true }));
        resizeInput();
        input.focus();
        showVoiceStatus("Transcript ready - edit, then send.", false);
      } else {
        const frames = typeof msg.frames === "number" ? msg.frames : 0;
        showVoiceStatus(`No speech detected (${frames} PCM frames captured).`, false);
      }
      return true;
    }
    // Anything else is shown verbatim and ends the take.
    showVoiceStatus(String(data), false);
    return true;
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
          releaseAudio(session);
          showVoiceStatus("The voice connection dropped.", true);
        }
      });
      source.connect(node);
      // The worklet renders silence, so reaching the destination is safe and
      // keeps the graph pulling on every engine.
      node.connect(ctx.destination);
      voice = session;
      clearInterim();
      ws.send("start");
      setRecording(true);
      statusBar.setRecording(true);
      showVoiceStatus("Recording - press the mic button again to stop.", false);
    } catch (error) {
      for (const track of stream.getTracks()) {
        track.stop();
      }
      // A socket or context that was created before the failure outlives
      // this function unless closed here; an open socket would hold the
      // server's session task until the connection times out.
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

  mic.addEventListener("click", () => {
    if (voice) {
      stopVoice();
    } else {
      void startVoice();
    }
  });
}
