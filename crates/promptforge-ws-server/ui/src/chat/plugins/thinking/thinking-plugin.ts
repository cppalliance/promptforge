import "./thinking.css";
import type { ChatPlugin, ContentBlock, PluginContext } from "../../core/types";
import { el } from "../../utils/dom";
import { renderSafeHTML } from "../../utils/html";
import { ICON_CHEVRON } from "../../utils/icons";

type ReasoningBlock = Extract<ContentBlock, { type: "reasoning" }>;

// collapsed: only the chevron + label row. preview (default while
// streaming): a fixed-height region capped at roughly four lines that
// scrolls internally, pinned to the newest line. expanded: full thinking.
type ThinkingMode = "collapsed" | "preview" | "expanded";

interface ThinkingState {
	mode: ThinkingMode;
	// A manual toggle is sticky: once the user clicks, auto open/collapse
	// behavior stops for this message.
	userToggled: boolean;
	autoCollapsed: boolean;
	// Preview scroll pinning: disengaged when the user scrolls up inside the
	// preview, re-engaged when they scroll back to the bottom.
	autoPin: boolean;
	cacheReasoning: string;
	cacheIsGenerating: boolean;
	latestBlock: ReasoningBlock;
	btn: HTMLButtonElement;
	liveEl: HTMLElement;
	contentEl: HTMLElement;
}

const PREVIEW_BOTTOM_TOLERANCE_PX = 8;

const LABEL_THINKING = "Thinking";
const LABEL_PREFILL = "Planning next moves";

const ENCRYPTED_REASONING_FALLBACK = "Thought process is hidden by the model provider.";

function getReasoningDisplayContent(block: ReasoningBlock): string {
	if (block.encrypted) return ENCRYPTED_REASONING_FALLBACK;
	return block.text;
}

export function ThinkingPlugin(): ChatPlugin {
	const stateMap = new WeakMap<HTMLElement, ThinkingState>();
	let blockSeq = 0;
	let prefillRow: HTMLElement | null = null;
	let unsubscribePrefill: (() => void) | null = null;

	const removePrefillRow = (): void => {
		prefillRow?.remove();
		prefillRow = null;
	};

	const pinPreviewToBottom = (state: ThinkingState): void => {
		state.contentEl.scrollTop = state.contentEl.scrollHeight;
	};

	const setMode = (state: ThinkingState, mode: ThinkingMode): void => {
		state.mode = mode;
		state.btn.setAttribute("aria-expanded", mode === "collapsed" ? "false" : "true");
		state.contentEl.hidden = mode === "collapsed";
		state.contentEl.classList.toggle("mur-think-content--preview", mode === "preview");
		state.contentEl.classList.toggle("mur-think-content--expanded", mode === "expanded");
		if (mode === "preview") {
			state.autoPin = true;
			pinPreviewToBottom(state);
		}
	};

	const renderContent = (state: ThinkingState): void => {
		if (state.mode === "collapsed") return;
		const displayContent = getReasoningDisplayContent(state.latestBlock);
		if (state.cacheReasoning === displayContent) return;
		renderSafeHTML(state.contentEl, displayContent);
		state.cacheReasoning = displayContent;
		if (state.mode === "preview" && state.autoPin) pinPreviewToBottom(state);
	};

	// State transitions are announced through a visually-hidden live region;
	// per-token text never is. The toggle label stays "Thinking" in every
	// state; the shimmer lives only on the prefill row.
	const syncStreamingChrome = (state: ThinkingState, isGenerating: boolean): void => {
		if (state.cacheIsGenerating === isGenerating) return;
		state.cacheIsGenerating = isGenerating;
		state.liveEl.textContent = isGenerating ? LABEL_THINKING : `${LABEL_THINKING} complete`;
	};

	// Collapsing shrinks the feed; capture the scroll position and restore it
	// after the layout change so the feed does not jump.
	const collapseWithScrollLock = (state: ThinkingState, containerEl: HTMLElement): void => {
		const scrollArea = containerEl.closest<HTMLElement>(".mur-chat-scroll-area");
		const scrollTop = scrollArea?.scrollTop ?? null;
		setMode(state, "collapsed");
		if (scrollArea === null || scrollTop === null) return;
		window.requestAnimationFrame(() => {
			scrollArea.scrollTop = scrollTop;
		});
	};

	const createBlock = (containerEl: HTMLElement, block: ReasoningBlock, isGenerating: boolean): ThinkingState => {
		const contentId = `mur-think-content-${blockSeq++}`;

		const btn = el("button", "mur-think-toggle", { type: "button" });
		btn.innerHTML = ICON_CHEVRON;
		btn.querySelector("svg")?.setAttribute("aria-hidden", "true");
		btn.setAttribute("aria-expanded", "false");
		btn.setAttribute("aria-controls", contentId);

		const labelEl = el("span", "mur-think-label", { textContent: LABEL_THINKING });
		btn.appendChild(labelEl);

		const contentEl = el("div", "mur-think-content");
		contentEl.id = contentId;
		contentEl.hidden = true;

		const liveEl = el("span", "mur-think-sr-only");
		liveEl.setAttribute("aria-live", "polite");

		const wrapper = el("div", "mur-think-wrapper", {}, [btn, contentEl, liveEl]);
		containerEl.innerHTML = "";
		containerEl.appendChild(wrapper);

		const state: ThinkingState = {
			mode: "collapsed",
			userToggled: false,
			autoCollapsed: false,
			autoPin: true,
			cacheReasoning: "",
			cacheIsGenerating: false,
		latestBlock: block,
		btn,
		liveEl,
		contentEl,
	};

		btn.addEventListener("click", () => {
			state.userToggled = true;
			if (state.mode === "expanded") {
				collapseWithScrollLock(state, containerEl);
				return;
			}
			setMode(state, "expanded");
			renderContent(state);
		});

		contentEl.addEventListener("scroll", () => {
			if (state.mode !== "preview") return;
			const distanceFromBottom = contentEl.scrollHeight - contentEl.clientHeight - contentEl.scrollTop;
			state.autoPin = distanceFromBottom <= PREVIEW_BOTTOM_TOLERANCE_PX;
		});

		if (isGenerating) {
			// First reasoning delta: auto-open into the capped preview.
			setMode(state, "preview");
			syncStreamingChrome(state, true);
		}

		return state;
	};

	// Prefill attach attempts are tokened: a new generation (or the end of
	// one) invalidates any attempt still waiting on the DOM.
	let prefillToken = 0;

	// One prefill attach attempt. "attached" and "stop" end the retry loop;
	// "retry" means the feed has not rendered the message element yet.
	const attachPrefillRow = (ctx: PluginContext, messageId: string): "attached" | "stop" | "retry" => {
		const engineState = ctx.engine.state;
		if (engineState.generatingMessageId !== messageId) return "stop";
		const message = engineState.messages.find((candidate) => candidate.id === messageId);
		if (!message) return "stop";
		// A block already streaming supersedes the prefill indicator.
		if (message.blocks.length > 0) return "stop";

		const target = ctx.container.querySelector(
			`.mur-message-assistant[data-message-id="${messageId}"]`,
		);
		if (!(target instanceof HTMLElement)) return "retry";

		const label = el("span", "mur-think-label mur-think-label--prefill", { textContent: LABEL_PREFILL });
		const row = el("div", "mur-think-prefill", {}, [label]);
		row.setAttribute("role", "status");
		target.appendChild(row);
		prefillRow = row;
		return "attached";
	};

	// Defers a callback past the current render pass; test environments
	// without requestAnimationFrame fall back to a short timeout.
	const defer = (fn: () => void): void => {
		if (typeof requestAnimationFrame === "function") {
			requestAnimationFrame(() => fn());
		} else {
			setTimeout(fn, 16);
		}
	};

	// Prefill indicator: shown the moment generation starts, before the first
	// reasoning or content token arrives. The selector notification fires
	// before the feed's hot render creates the message element, so the
	// attach is retried across a few frames until the element exists.
	const showPrefillRow = (ctx: PluginContext, messageId: string): void => {
		const token = ++prefillToken;
		const tryAttach = (remaining: number): void => {
			if (token !== prefillToken) return;
			if (attachPrefillRow(ctx, messageId) !== "retry" || remaining <= 0) return;
			defer(() => tryAttach(remaining - 1));
		};
		// The store notifies selectors before the hot render, both in the
		// same synchronous set; a microtask lands after that render.
		queueMicrotask(() => tryAttach(10));
	};

	return {
		name: "thinking",
		ownsEmptyLoadingState: true,

		onMount: (ctx) => {
			unsubscribePrefill = ctx.engine.onChange(
				(engineState) => engineState.generatingMessageId,
				(generatingMessageId) => {
					prefillToken++;
					removePrefillRow();
					if (generatingMessageId === null) return;
					showPrefillRow(ctx, generatingMessageId);
				},
			);
		},

		destroy: () => {
			prefillToken++;
			removePrefillRow();
			unsubscribePrefill?.();
			unsubscribePrefill = null;
		},

		onBlockRender: (block, containerEl, isGenerating) => {
			// Any block rendering into the message that carries the prefill row
			// supersedes it, whatever the block type.
			if (prefillRow?.parentElement?.contains(containerEl)) {
				removePrefillRow();
			}

			if (block.type !== "reasoning") return false;

			// A real reasoning block supersedes the prefill indicator.
			removePrefillRow();

			let state = stateMap.get(containerEl);
			if (!state) {
				state = createBlock(containerEl, block, isGenerating);
				stateMap.set(containerEl, state);
			}
			state.latestBlock = block;

			const wasGenerating = state.cacheIsGenerating;
			syncStreamingChrome(state, isGenerating);

			if (wasGenerating && !isGenerating && !state.autoCollapsed) {
				// The first content token ends the reasoning stream: collapse
				// once to the label row unless the user took over manually.
				state.autoCollapsed = true;
				if (!state.userToggled && state.mode !== "collapsed") {
					collapseWithScrollLock(state, containerEl);
				}
			}

			renderContent(state);
			return true;
		},
	};
}
