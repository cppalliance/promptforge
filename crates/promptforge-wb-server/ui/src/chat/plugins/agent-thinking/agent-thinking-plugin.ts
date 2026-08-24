import "./agent-thinking.css";
import type { ChatPlugin } from "../../core/types";
import { el } from "../../utils/dom";

export interface AgentThinkingPluginConfig {
	previewLines?: number;
}

interface AgentThinkingState {
	expanded: boolean;
	expandable: boolean;
	explicitExpandable: boolean;
	contentCache: string;
	measureFrame: number | null;
	previewEl: HTMLElement;
	textEl: HTMLElement;
}

const DEFAULT_PREVIEW_LINES = 3;
const ENCRYPTED_REASONING_FALLBACK = "Thought process is hidden by the model provider.";

export function AgentThinkingPlugin(config: AgentThinkingPluginConfig = {}): ChatPlugin {
	const stateMap = new WeakMap<HTMLElement, AgentThinkingState>();
	const previewLines = Math.max(1, Math.floor(config.previewLines ?? DEFAULT_PREVIEW_LINES));

	return {
		name: "agent-thinking",
		onBlockRender: (block, containerEl) => {
			if (block.type !== "reasoning") return false;

			const content = reasoningContent(block);
			if (content.trim().length === 0) return false;

			let state = stateMap.get(containerEl);
			if (!state) {
				state = createState(previewLines);
				containerEl.replaceChildren(state.previewEl);
				stateMap.set(containerEl, state);
			}

			containerEl.className = "mur-content-block mur-block-reasoning mur-agent-think";
			state.previewEl.style.setProperty("--mur-agent-think-preview-lines", String(previewLines));
			if (state.contentCache !== content) {
				state.textEl.textContent = content;
				state.contentCache = content;
				state.explicitExpandable = countExplicitLines(content) > previewLines;
				state.expandable = state.explicitExpandable;
			}
			syncState(state);
			if (!state.explicitExpandable && !state.expanded) queueMeasure(state);

			return true;
		},
	};
}

function createState(previewLines: number): AgentThinkingState {
	const textEl = el("span", "mur-agent-think-text");
	const previewEl = el("div", "mur-agent-think-preview", null, [textEl]);
	previewEl.style.setProperty("--mur-agent-think-preview-lines", String(previewLines));

	const state: AgentThinkingState = {
		expanded: false,
		expandable: false,
		explicitExpandable: false,
		contentCache: "",
		measureFrame: null,
		previewEl,
		textEl,
	};

	previewEl.addEventListener("click", () => toggleExpanded(state));
	previewEl.addEventListener("keydown", (event) => {
		if (event.key !== "Enter" && event.key !== " ") return;
		if (!state.expandable) return;
		event.preventDefault();
		toggleExpanded(state);
	});

	syncState(state);
	return state;
}

function toggleExpanded(state: AgentThinkingState): void {
	if (!state.expandable) return;
	state.expanded = !state.expanded;
	syncState(state);
}

function syncState(state: AgentThinkingState): void {
	if (!state.expandable) state.expanded = false;

	state.previewEl.dataset.expandable = String(state.expandable);
	state.previewEl.dataset.expanded = String(state.expanded);

	if (state.expandable) {
		state.previewEl.setAttribute("role", "button");
		state.previewEl.tabIndex = 0;
		state.previewEl.setAttribute("aria-expanded", String(state.expanded));
		state.previewEl.setAttribute("aria-label", "Toggle reasoning");
		return;
	}

	state.previewEl.removeAttribute("role");
	state.previewEl.removeAttribute("tabindex");
	state.previewEl.removeAttribute("aria-expanded");
	state.previewEl.removeAttribute("aria-label");
}

function queueMeasure(state: AgentThinkingState): void {
	const win = state.previewEl.ownerDocument.defaultView;
	const requestFrame =
		win?.requestAnimationFrame?.bind(win) ??
		(typeof requestAnimationFrame === "function" ? requestAnimationFrame : undefined);
	const cancelFrame =
		win?.cancelAnimationFrame?.bind(win) ??
		(typeof cancelAnimationFrame === "function" ? cancelAnimationFrame : undefined);

	if (state.measureFrame !== null && cancelFrame) cancelFrame(state.measureFrame);

	if (!requestFrame) {
		measureExpandable(state);
		return;
	}

	state.measureFrame = requestFrame(() => {
		state.measureFrame = null;
		measureExpandable(state);
	});
}

function measureExpandable(state: AgentThinkingState): void {
	if (state.expanded || state.explicitExpandable) return;

	const measuredExpandable = state.textEl.scrollHeight > state.textEl.clientHeight + 1;
	if (state.expandable === measuredExpandable) return;

	state.expandable = measuredExpandable;
	syncState(state);
}

function reasoningContent(block: { text: string; encrypted?: boolean }): string {
	if (block.encrypted) return ENCRYPTED_REASONING_FALLBACK;
	return block.text;
}

function countExplicitLines(text: string): number {
	return text.split(/\r\n|\r|\n/).length;
}
