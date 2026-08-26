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
	labelEl: HTMLElement;
	liveEl: HTMLElement;
	contentEl: HTMLElement;
}

const GRACE_DELAY_MS = 500;
const PREVIEW_BOTTOM_TOLERANCE_PX = 8;

const LABEL_IDLE = "Thinking";
const LABEL_STREAMING = "Planning next moves...";

const ENCRYPTED_REASONING_FALLBACK = "<i>Thought process is hidden by the model provider.</i>";

function getReasoningDisplayContent(block: ReasoningBlock): string {
	if (block.encrypted) return ENCRYPTED_REASONING_FALLBACK;
	return block.text;
}

export function ThinkingPlugin(): ChatPlugin {
	const stateMap = new WeakMap<HTMLElement, ThinkingState>();
	let blockSeq = 0;
	let graceTimer: number | undefined;
	let graceRow: HTMLElement | null = null;
	let unsubscribeGrace: (() => void) | null = null;

	const clearGraceTimer = (): void => {
		if (graceTimer === undefined) return;
		window.clearTimeout(graceTimer);
		graceTimer = undefined;
	};

	const removeGraceRow = (): void => {
		graceRow?.remove();
		graceRow = null;
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
	// per-token text never is.
	const syncStreamingChrome = (state: ThinkingState, isGenerating: boolean): void => {
		if (state.cacheIsGenerating === isGenerating) return;
		state.cacheIsGenerating = isGenerating;
		const label = isGenerating ? LABEL_STREAMING : LABEL_IDLE;
		state.labelEl.textContent = label;
		state.labelEl.classList.toggle("mur-think-label--streaming", isGenerating);
		state.liveEl.textContent = label;
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

		const labelEl = el("span", "mur-think-label", { textContent: LABEL_IDLE });
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
			labelEl,
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

	// Grace-period loader: a synthetic streaming row, shown only when
	// generation runs ~500ms with no reasoning block, so fast responses never
	// flicker it.
	const showGraceRow = (ctx: PluginContext, messageId: string): void => {
		const engineState = ctx.engine.state;
		if (engineState.generatingMessageId !== messageId) return;
		const message = engineState.messages.find((candidate) => candidate.id === messageId);
		if (!message || message.blocks.length > 0) return;

		const messages = ctx.container.querySelectorAll(".mur-message-assistant");
		const target = messages.item(messages.length - 1);
		if (!(target instanceof HTMLElement)) return;

		const label = el("span", "mur-think-label mur-think-label--streaming", { textContent: LABEL_STREAMING });
		const row = el("div", "mur-think-grace", {}, [label]);
		row.setAttribute("role", "status");
		target.appendChild(row);
		graceRow = row;
	};

	return {
		name: "thinking",

		onMount: (ctx) => {
			unsubscribeGrace = ctx.engine.onChange(
				(engineState) => engineState.generatingMessageId,
				(generatingMessageId) => {
					clearGraceTimer();
					removeGraceRow();
					if (generatingMessageId === null) return;
					graceTimer = window.setTimeout(() => {
						graceTimer = undefined;
						showGraceRow(ctx, generatingMessageId);
					}, GRACE_DELAY_MS);
				},
			);
		},

		destroy: () => {
			clearGraceTimer();
			removeGraceRow();
			unsubscribeGrace?.();
			unsubscribeGrace = null;
		},

		onBlockRender: (block, containerEl, isGenerating) => {
			// Any block rendering into the message that carries the grace row
			// supersedes the loader, whatever the block type.
			if (graceRow?.parentElement?.contains(containerEl)) {
				clearGraceTimer();
				removeGraceRow();
			}

			if (block.type !== "reasoning") return false;

			// A real reasoning block supersedes the grace-period loader.
			clearGraceTimer();
			removeGraceRow();

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
