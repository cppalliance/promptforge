// murm-ui's own styles, bundled by esbuild into dist/app.css. Sidebar and
// dropdown styles are skipped: the workbench disables the murm sidebar and
// no plugin renders dropdowns.
import "./chat/styles/base.css";
import "./chat/styles/feed.css";
import "./chat/styles/input.css";

import type { ChatPlugin } from "./chat/core/types";
import { ChatUI } from "./chat/main";
import { MemoryStorage } from "./memory-storage";
import { setupVoice } from "./voice";
import { WorkbenchProvider } from "./workbench-provider";

const pickerEl = document.getElementById("model-picker") as HTMLSelectElement;
const descriptionEl = document.getElementById("model-description") as HTMLDivElement;

interface ModelEntry {
  id: string;
  description?: string;
}

function selectedModel(): string {
  return pickerEl.value;
}

// The mic button joins murm-ui's composer through the plugin seam; the
// interim transcript and voice status sit above the form.
const voicePlugin: ChatPlugin = {
  name: "voice",
  onInputMount({ container, form, input }) {
    const mic = document.createElement("button");
    mic.type = "button";
    mic.className = "mic-button mur-form-icon-btn";
    mic.title = "Push to talk";
    mic.setAttribute("aria-label", "Push to talk");
    mic.setAttribute("aria-pressed", "false");
    mic.innerHTML =
      '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="2" width="6" height="12" rx="3"></rect><path d="M5 10a7 7 0 0 0 14 0"></path><line x1="12" y1="19" x2="12" y2="22"></line></svg>';
    form.insertBefore(mic, form.querySelector(".mur-form-footer-right"));

    const formContainer = container.querySelector(".mur-chat-form-container");
    if (!formContainer) {
      throw new Error("DOM Error: .mur-chat-form-container not found inside the container.");
    }
    const interim = document.createElement("div");
    interim.className = "interim";
    interim.setAttribute("aria-live", "polite");
    const status = document.createElement("div");
    status.className = "voice-status";
    status.setAttribute("role", "status");
    status.setAttribute("aria-live", "polite");
    formContainer.insertBefore(interim, form);
    formContainer.appendChild(status);

    setupVoice({ mic, interim, status, input });
  },
  // With no model selected there is nothing to send to; the old UI disabled
  // the send button in the same situation.
  isSubmitBlocked: () => !selectedModel(),
};

const chat = new ChatUI({
  container: ".mur-app",
  provider: new WorkbenchProvider(),
  storage: new MemoryStorage(),
  enableSidebar: false,
  routing: false,
  fullscreen: false,
  plugins: () => [voicePlugin],
});

function applyModel(): void {
  chat.engine.setRequestDefaults({ options: { model: selectedModel() } });
}

function showDescription(): void {
  const option = pickerEl.selectedOptions[0];
  descriptionEl.textContent = (option && option.dataset.description) || "";
}

async function loadModels(): Promise<void> {
  try {
    const response = await fetch("/v1/models");
    if (!response.ok) {
      throw new Error(`GET /v1/models answered ${response.status}`);
    }
    const catalog = (await response.json()) as { data?: ModelEntry[] };
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
    applyModel();
  } catch (error) {
    pickerEl.textContent = "";
    pickerEl.appendChild(new Option("Model catalog unavailable", ""));
    pickerEl.disabled = true;
    descriptionEl.textContent = `Could not load the model catalog: ${(error as Error).message}`;
    descriptionEl.classList.add("error");
  }
}

pickerEl.addEventListener("change", () => {
  showDescription();
  applyModel();
});

void loadModels();
