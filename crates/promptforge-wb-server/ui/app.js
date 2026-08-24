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
