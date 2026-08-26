// murm-ui's own styles, bundled by esbuild into dist/app.css. Sidebar and
// dropdown styles are skipped: the workshop disables the murm sidebar and
// no plugin renders dropdowns.
import "./chat/styles/base.css";
import "./chat/styles/feed.css";
import "./chat/styles/input.css";
import "dockview/dist/styles/dockview.css";

import { createDockview, themeDark } from "dockview";
import type { IContentRenderer } from "dockview";

import type { ChatPlugin } from "./chat/core/types";
import { ChatUI } from "./chat/main";
import { ThinkingPlugin } from "./chat/plugins/thinking/thinking-plugin";
import { MemoryStorage } from "./memory-storage";
import { StatusBar } from "./status-bar";
import { setupVoice, type VoiceHandle } from "./voice";
import { WorkshopProvider } from "./workshop-provider";
import { type CatalogModel, WorkshopSocket } from "./workshop-socket";

const pickerEl = document.getElementById("model-picker") as HTMLSelectElement;
const descriptionEl = document.getElementById("model-description") as HTMLDivElement;

// One persistent socket carries chat frames upstream and every downstream
// JSON frame - chat replies and the observer's status updates, which the
// status bar renders as they arrive.
const statusBarRoot = document.querySelector(".status-bar") as HTMLElement | null;
if (!statusBarRoot) {
  throw new Error("DOM Error: .status-bar not found in the page.");
}
const statusBar = new StatusBar(statusBarRoot);
const workshopSocket = new WorkshopSocket();
workshopSocket.onStatus((frame) => statusBar.render(frame));
// A dropped socket means every in-flight status is stale; the bar returns
// to its reconnecting state until the observer speaks again.
workshopSocket.onDisconnect(() => statusBar.reset());
workshopSocket.connect();

function selectedModel(): string {
  return pickerEl.value;
}

// The mic button joins murm-ui's composer through the plugin seam; voice
// messages paint the status bar directly.
let voiceHandle: VoiceHandle | null = null;
const voicePlugin: ChatPlugin = {
  name: "voice",
  onInputMount({ form, input }) {
    const mic = document.createElement("button");
    mic.type = "button";
    mic.className = "voice-mic mur-form-icon-btn";
    mic.title = "Push to talk";
    mic.setAttribute("aria-label", "Push to talk");
    mic.setAttribute("aria-pressed", "false");
    mic.innerHTML =
      '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="2" width="6" height="12" rx="3"></rect><path d="M5 10a7 7 0 0 0 14 0"></path><line x1="12" y1="19" x2="12" y2="22"></line></svg>';
    form.insertBefore(mic, form.querySelector(".mur-form-footer-right"));

    voiceHandle = setupVoice({ mic, input }, statusBar);
  },
  onUserSubmit() {
    voiceHandle?.discardIfRecording();
  },
  // With no model selected there is nothing to send to; the old UI disabled
  // the send button in the same situation.
  isSubmitBlocked: () => !selectedModel(),
};

// The chat lives in dockview's single panel; the panel infrastructure is
// what later stages hang the file tree and editor panes on. The tab bar is
// hidden in style.css: with exactly one panel it is chrome, not information.
class ChatPanel implements IContentRenderer {
  readonly element = document.createElement("div");

  constructor() {
    this.element.className = "chat-panel";
  }

  init(): void {
    const template = document.getElementById("chat-panel") as HTMLTemplateElement;
    this.element.appendChild(template.content.cloneNode(true));
  }
}

const dockEl = document.getElementById("dock") as HTMLDivElement;
const dock = createDockview(dockEl, {
  createComponent: () => new ChatPanel(),
  theme: themeDark,
  singleTabMode: "fullwidth",
  disableFloatingGroups: true,
  hideBorders: true,
  locked: true,
  noPanelsOverlay: "emptyGroup",
});
dock.addPanel({ id: "chat", component: "chat", title: "Chat" });

const chatContainer = dockEl.querySelector(".mur-app");
if (!chatContainer) {
  throw new Error("DOM Error: the chat panel did not mount its .mur-app container.");
}

const chat = new ChatUI({
  container: chatContainer as HTMLElement,
  provider: new WorkshopProvider(workshopSocket),
  storage: new MemoryStorage(),
  enableSidebar: false,
  routing: false,
  fullscreen: false,
  plugins: () => [voicePlugin, ThinkingPlugin()],
});

function applyModel(): void {
  chat.engine.setRequestDefaults({ options: { model: selectedModel() } });
}

function showDescription(): void {
  const option = pickerEl.selectedOptions[0];
  descriptionEl.textContent = (option && option.dataset.description) || "";
}

// Rebuilds the model picker from a catalog, keeping the user's selection
// when it survives the refresh. Used by the boot fetch and by the pushed
// catalogs the server sends when the gateway comes back.
function renderModels(entries: CatalogModel[]): void {
  const previous = pickerEl.value;
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
  if (entries.some((entry) => entry.id === previous)) {
    pickerEl.value = previous;
  }
  pickerEl.disabled = false;
  descriptionEl.classList.remove("sidebar__model-description--error");
  showDescription();
  applyModel();
}

async function loadModels(): Promise<void> {
  try {
    const response = await fetch("/v1/models");
    if (!response.ok) {
      throw new Error(`GET /v1/models answered ${response.status}`);
    }
    const catalog = (await response.json()) as { data?: CatalogModel[] };
    renderModels(Array.isArray(catalog.data) ? catalog.data : []);
  } catch (error) {
    pickerEl.textContent = "";
    pickerEl.appendChild(new Option("Model catalog unavailable", ""));
    pickerEl.disabled = true;
    descriptionEl.textContent = `Could not load the model catalog: ${(error as Error).message}`;
    descriptionEl.classList.add("sidebar__model-description--error");
  }
}

// A pushed catalog means the gateway returned after an outage; refresh the
// picker in place so a boot-time "Model catalog unavailable" heals itself.
workshopSocket.onModels(renderModels);

pickerEl.addEventListener("change", () => {
  showDescription();
  applyModel();
});

void loadModels();
