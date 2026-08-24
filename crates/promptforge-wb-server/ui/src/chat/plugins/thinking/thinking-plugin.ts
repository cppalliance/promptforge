import "./thinking.css";
import type { ChatPlugin } from "../../core/types";
import { el } from "../../utils/dom";
import { renderSafeHTML } from "../../utils/html";
import { ICON_CHEVRON } from "../../utils/icons";

interface ThinkingState {
	isExpanded: boolean;
	cacheReasoning: string;
	cacheIsGenerating: boolean;
	contentEl: HTMLElement;
	btnSpan: HTMLElement;
}

const ENCRYPTED_REASONING_FALLBACK = "<i>Thought process is hidden by the model provider.</i>";

function getReasoningDisplayContent(block: { text: string; encrypted?: boolean }): string {
	if (block.encrypted) return ENCRYPTED_REASONING_FALLBACK;
	return block.text;
}

export function ThinkingPlugin(): ChatPlugin {
	const stateMap = new WeakMap<HTMLElement, ThinkingState>();

	return {
		name: "thinking",
		onBlockRender: (block, containerEl, isGenerating) => {
			if (block.type !== "reasoning") return false;

			let state = stateMap.get(containerEl);

			if (!state) {
				const btn = el("button", "mur-think-toggle", {
					innerHTML: ICON_CHEVRON + "<span>Thought Process</span>",
				});

				const btnSpan = btn.querySelector("span") as HTMLElement;
				btn.setAttribute("aria-expanded", "false");

				const contentEl = el("div", "mur-think-content");
				contentEl.hidden = true;
				const wrapper = el("div", "mur-think-wrapper", {}, [btn, contentEl]);

				containerEl.innerHTML = "";
				containerEl.appendChild(wrapper);

				state = {
					isExpanded: false,
					cacheReasoning: "",
					cacheIsGenerating: false,
					contentEl,
					btnSpan,
				};

				btn.onclick = () => {
					state!.isExpanded = !state!.isExpanded;
					contentEl.hidden = !state!.isExpanded;
					btn.setAttribute("aria-expanded", String(state!.isExpanded));

					const displayContent = getReasoningDisplayContent(block);

					if (state!.isExpanded && state!.cacheReasoning !== displayContent) {
						renderSafeHTML(contentEl, displayContent);
						state!.cacheReasoning = displayContent;
					}
				};

				stateMap.set(containerEl, state);
			}

			if (state.cacheIsGenerating !== isGenerating) {
				state.btnSpan.textContent = isGenerating ? "Thinking..." : "Thought Process";
				state.cacheIsGenerating = isGenerating;
			}

			const displayContent = getReasoningDisplayContent(block);

			if (state.isExpanded && state.cacheReasoning !== displayContent) {
				renderSafeHTML(state.contentEl, displayContent);
				state.cacheReasoning = displayContent;
			}

			return true;
		},
	};
}
