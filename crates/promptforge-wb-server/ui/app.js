"use strict";

const md = window.markdownit({
  breaks: true,
  linkify: true,
});

const messagesEl = document.getElementById("messages");
const composerEl = document.getElementById("composer");
const inputEl = document.getElementById("input");
const sendEl = document.getElementById("send");
const pickerEl = document.getElementById("model-picker");
const descriptionEl = document.getElementById("model-description");
const micEl = document.getElementById("mic");
const voiceStatusEl = document.getElementById("voice-status");

// OpenAI-shaped history: [{role, content}, ...], sent verbatim to /chat.
const history = [];
let streaming = false;

function selectedModel() {
  return pickerEl.value;
}

function scrollToBottom() {
  messagesEl.scrollTop = messagesEl.scrollHeight;
}

function addBubble(role, text) {
  const message = document.createElement("div");
  message.className = `message ${role}`;
  const bubble = document.createElement("div");
  bubble.className = "bubble";
  bubble.textContent = text;
  message.appendChild(bubble);
  messagesEl.appendChild(message);
  scrollToBottom();
  return bubble;
}

function addErrorBubble(text) {
  addBubble("error", text);
}

function setStreaming(next) {
  streaming = next;
  sendEl.disabled = next || !pickerEl.value;
  inputEl.readOnly = next;
}

function autoResize() {
  inputEl.style.height = "auto";
  inputEl.style.height = `${Math.min(inputEl.scrollHeight, 200)}px`;
}

async function loadModels() {
  try {
    const response = await fetch("/v1/models");
    if (!response.ok) {
      throw new Error(`GET /v1/models answered ${response.status}`);
    }
    const catalog = await response.json();
    const entries = Array.isArray(catalog.data) ? catalog.data : [];
    pickerEl.textContent = "";
    if (entries.length === 0) {
      pickerEl.appendChild(new Option("No models available", ""));
      pickerEl.disabled = true;
      return;
    }
    for (const entry of entries) {
      const option = new Option(entry.id, entry.id);
      option.dataset.description = entry.description || "";
      pickerEl.appendChild(option);
    }
    pickerEl.disabled = false;
    showDescription();
    sendEl.disabled = false;
  } catch (error) {
    pickerEl.textContent = "";
    pickerEl.appendChild(new Option("Model catalog unavailable", ""));
    pickerEl.disabled = true;
    addErrorBubble(`Could not load the model catalog: ${error.message}`);
  }
}

function showDescription() {
  const option = pickerEl.selectedOptions[0];
  descriptionEl.textContent = (option && option.dataset.description) || "";
}

async function send(text) {
  history.push({ role: "user", content: text });
  addBubble("user", text);

  const message = document.createElement("div");
  message.className = "message assistant streaming";
  const bubble = document.createElement("div");
  bubble.className = "bubble";
  message.appendChild(bubble);
  messagesEl.appendChild(message);
  scrollToBottom();

  setStreaming(true);
  let assembled = "";
  try {
    const response = await fetch("/chat", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        model: selectedModel(),
        messages: history,
        stream: true,
      }),
    });
    if (!response.ok) {
      const detail = await response.text();
      throw new Error(`POST /chat answered ${response.status}: ${detail}`);
    }
    await streamInto(response.body, (delta) => {
      assembled += delta;
      bubble.innerHTML = md.render(assembled);
      scrollToBottom();
    });
    if (assembled.length > 0) {
      history.push({ role: "assistant", content: assembled });
    }
  } catch (error) {
    message.remove();
    addErrorBubble(error.message);
  } finally {
    message.classList.remove("streaming");
    setStreaming(false);
    inputEl.focus();
  }
}

// Reads an SSE body, invoking onDelta for each choices[0].delta.content.
// fetch + ReadableStream rather than EventSource because /chat is a POST.
async function streamInto(body, onDelta) {
  const reader = body.pipeThrough(new TextDecoderStream()).getReader();
  let buffer = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) {
      break;
    }
    buffer += value;
    const events = buffer.split("\n\n");
    buffer = events.pop();
    for (const event of events) {
      const data = event
        .split("\n")
        .filter((line) => line.startsWith("data:"))
        .map((line) => line.slice(5).replace(/^ /, ""))
        .join("\n");
      if (data === "") {
        continue;
      }
      if (data === "[DONE]") {
        return;
      }
      let chunk;
      try {
        chunk = JSON.parse(data);
      } catch {
        // A non-JSON data payload carries no chat delta; skip it and keep
        // reading the stream.
        continue;
      }
      const delta = chunk.choices?.[0]?.delta?.content;
      if (typeof delta === "string") {
        onDelta(delta);
      }
    }
  }
  // A stream that ends without [DONE] was truncated mid-way (design entry
  // 15): the partial text stays on screen and the cursor stops blinking.
}

// Voice capture: the active session's {ws, ctx, source, node, stream}, or
// null while idle. One session at a time; the mic button toggles it.
let voice = null;
let voiceStatusTimer = 0;

function showVoiceStatus(text, isError) {
  voiceStatusEl.textContent = text;
  voiceStatusEl.classList.toggle("error", Boolean(isError));
  voiceStatusEl.classList.add("visible");
  clearTimeout(voiceStatusTimer);
  voiceStatusTimer = setTimeout(() => {
    voiceStatusEl.classList.remove("visible");
  }, 8000);
}

function setRecording(next) {
  micEl.classList.toggle("recording", next);
  micEl.setAttribute("aria-pressed", String(next));
  micEl.title = next ? "Stop recording" : "Push to talk";
}

// Tears down a session's audio half. The socket half is closed by the
// caller, after any in-flight "stop" reply has had a chance to arrive.
function releaseAudio(session) {
  session.node.port.onmessage = null;
  session.source.disconnect();
  session.node.disconnect();
  for (const track of session.stream.getTracks()) {
    track.stop();
  }
  // Best effort: a failed close leaves nothing the page can still act on.
  session.ctx.close().catch(() => {});
}

async function startVoice() {
  if (!navigator.mediaDevices?.getUserMedia || !window.AudioContext || !window.WebSocket) {
    showVoiceStatus("Voice capture is not available in this browser.", true);
    return;
  }
  let stream;
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
      error && error.name === "NotAllowedError"
        ? "microphone permission denied"
        : `microphone unavailable: ${error.message || error}`;
    showVoiceStatus(detail, true);
    return;
  }
  let ws;
  let ctx;
  try {
    ws = new WebSocket(
      `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/voice`,
    );
    ws.binaryType = "arraybuffer";
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve, { once: true });
      ws.addEventListener("error", () => reject(new Error("the /voice socket failed to open")), {
        once: true,
      });
    });
    // The context resamples the mic stream to 16 kHz before the worklet
    // sees it, so the wire format is 16 kHz mono f32 on any device.
    ctx = new AudioContext({ sampleRate: 16000 });
    await ctx.audioWorklet.addModule("/pcm-worklet.js");
    const source = ctx.createMediaStreamSource(stream);
    const node = new AudioWorkletNode(ctx, "pcm-capture");
    const session = { ws, ctx, source, node, stream };
    node.port.onmessage = (event) => {
      if (voice === session && ws.readyState === WebSocket.OPEN) {
        ws.send(event.data);
      }
    };
    ws.addEventListener("message", (event) => {
      showVoiceStatus(voiceReplyNote(event.data), false);
      ws.close();
    });
    ws.addEventListener("close", () => {
      if (voice === session) {
        voice = null;
        setRecording(false);
        releaseAudio(session);
        showVoiceStatus("The voice connection dropped.", true);
      }
    });
    source.connect(node);
    // The worklet renders silence, so reaching the destination is safe and
    // keeps the graph pulling on every engine.
    node.connect(ctx.destination);
    voice = session;
    ws.send("start");
    setRecording(true);
    showVoiceStatus("Recording - press the mic button again to stop.", false);
  } catch (error) {
    for (const track of stream.getTracks()) {
      track.stop();
    }
    // A socket or context that was created before the failure outlives this
    // function unless closed here; an open socket would hold the server's
    // session task until the connection times out.
    if (ws) {
      ws.close();
    }
    if (ctx) {
      ctx.close().catch(() => {});
    }
    showVoiceStatus(`Voice capture failed: ${error.message || error}`, true);
  }
}

// Renders the server's reply to "stop" as a human note. The reply is JSON
// ({"frames":N}); anything else is shown verbatim.
function voiceReplyNote(data) {
  try {
    const parsed = JSON.parse(data);
    if (typeof parsed.frames === "number") {
      return `Captured ${parsed.frames} PCM frames (16 kHz mono).`;
    }
  } catch {
    // Not JSON: fall through and show the raw text.
  }
  return String(data);
}

function stopVoice() {
  const session = voice;
  voice = null;
  setRecording(false);
  if (!session) {
    return;
  }
  releaseAudio(session);
  const { ws } = session;
  if (ws.readyState === WebSocket.OPEN) {
    ws.send("stop");
    // The frame-count reply closes the socket from the message listener;
    // this is the fallback if the reply never comes.
    setTimeout(() => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.close();
      }
    }, 2000);
  }
}

micEl.addEventListener("click", () => {
  if (voice) {
    stopVoice();
  } else {
    startVoice();
  }
});

composerEl.addEventListener("submit", (event) => {
  event.preventDefault();
  const text = inputEl.value.trim();
  if (text === "" || streaming || !selectedModel()) {
    return;
  }
  inputEl.value = "";
  autoResize();
  send(text);
});

inputEl.addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    composerEl.requestSubmit();
  }
});

inputEl.addEventListener("input", autoResize);
pickerEl.addEventListener("change", showDescription);

loadModels();
inputEl.focus();
