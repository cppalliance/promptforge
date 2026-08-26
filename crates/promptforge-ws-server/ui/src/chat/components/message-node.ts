import type { ActionButtonDef, BlockRenderContext, ContentBlock, Message, RenderConfig } from "../core/types";
import { StreamingMarkdownRenderer } from "../markdown-blocks";
import { el } from "../utils/dom";

const MARKDOWN_THROTTLE_MS = 70;

interface BlockState {
	container: HTMLElement;
	textCache: string | null;
	renderSeq: number;
	markdown: StreamingMarkdownRenderer | null;
	timer?: number;
}

export class MessageNode {
	public readonly el: HTMLElement;

	private blocksContainer: HTMLElement;
	private loadingEl?: HTMLElement;
	private errorEl?: HTMLElement;
	private actionsEl?: HTMLElement;

	private activeBlocks = new Map<string, BlockState>();

	private cacheError: string | null = null;
	private cacheIsGenerating: boolean = false;
	private cacheActionsVisible: boolean = false;
	private actionsInitialized: boolean = false;
	private currentMsg: Message | null = null;
	private isDestroyed = false;

	constructor(
		msg: Message,
		private config: RenderConfig,
	) {
		this.el = document.createElement("div");
		this.el.className = `mur-message mur-message-${msg.role}`;
		if (msg.role === "assistant") {
			this.el.setAttribute("role", "article");
			this.el.setAttribute("aria-label", "AI response");
		}

		this.blocksContainer = el("div", "mur-message-blocks-wrapper");
		this.el.appendChild(this.blocksContainer);
	}

	public update(msg: Message, isGenerating: boolean, error: string | null, messages: readonly Message[]) {
		this.currentMsg = msg;

		if (this.cacheIsGenerating !== isGenerating) {
			this.el.classList.toggle("mur-generating", isGenerating);
			this.cacheIsGenerating = isGenerating;
		}

		this.renderBlocks(msg, isGenerating, messages);
		this.renderLoading(msg, isGenerating, error);
		this.renderActions(msg, isGenerating);
		this.renderError(error);
	}

	public destroy() {
		this.isDestroyed = true;
		for (const state of this.activeBlocks.values()) {
			if (state.timer !== undefined) clearTimeout(state.timer);
			state.markdown?.destroy();
		}
		this.el.remove();
	}

	private renderLoading(msg: Message, isGenerating: boolean, error: string | null) {
		const hasVisibleBlocks = this.activeBlocks.size > 0;
		// A plugin (e.g. thinking) may own the empty assistant loading state;
		// when one does, the generic three-dot fallback never renders.
		const pluginOwnsLoading = this.config.plugins.some((plugin) => plugin.ownsEmptyLoadingState === true);
		const isLoading = isGenerating && !error && msg.role === "assistant" && !hasVisibleBlocks && !pluginOwnsLoading;

		if (isLoading) {
			if (!this.loadingEl) {
				this.loadingEl = el("div", "mur-message-loading", {
					innerHTML: `<span class="mur-loading-dot"></span><span class="mur-loading-dot"></span><span class="mur-loading-dot"></span>`,
				});
				this.el.appendChild(this.loadingEl);
			}
		} else if (this.loadingEl) {
			this.loadingEl.remove();
			this.loadingEl = undefined;
		}
	}

	private renderBlocks(msg: Message, isGenerating: boolean, messages: readonly Message[]) {
		const visibleBlockIds = new Set<string>();
		let displayIndex = 0;

		for (let i = 0; i < msg.blocks.length; i++) {
			const block = msg.blocks[i];
			const isLastBlock = i === msg.blocks.length - 1;
			const isGeneratingBlock = isGenerating && isLastBlock;

			let state = this.activeBlocks.get(block.id);
			let isNew = false;

			if (!state) {
				const container = el("div", `mur-content-block mur-block-${block.type}`);
				container.dataset.blockId = block.id;
				state = { container, textCache: null, renderSeq: 0, markdown: null };
				isNew = true;
			}
			const container = state.container;

			let handledByPlugin = false;
			let blockRenderCtx: BlockRenderContext | undefined;
			for (const plugin of this.config.plugins) {
				if (!plugin.onBlockRender) continue;

				blockRenderCtx ??= { message: msg, messages, blockIndex: i };
				try {
					if (plugin.onBlockRender(block, container, isGeneratingBlock, blockRenderCtx)) {
						handledByPlugin = true;
						break;
					}
				} catch (error) {
					console.error(`Plugin "${plugin.name}" failed during onBlockRender`, error);
				}
			}

			if (!handledByPlugin) {
				switch (block.type) {
					case "reasoning":
						// Fallback behavior: If no plugin (like ThinkingPlugin) handles reasoning blocks,
						// we skip them entirely. No DOM node will be added or retained.
						continue;
					case "text":
						this.renderTextBlock(block, state, isGeneratingBlock);
						break;
					case "file":
						this.renderFileBlock(block, container);
						break;
					case "tool_call":
						container.textContent = `🛠 Tool Call: ${block.name} (${block.status})`;
						container.className = `mur-content-block mur-block-tool mur-tool-${block.status}`;
						break;
					case "tool_result":
					case "artifact":
						// These are background/contextual blocks not meant for direct rendering.
						continue;
				}
			}

			// If we didn't 'continue', it means the block is visible
			visibleBlockIds.add(block.id);

			if (isNew) {
				this.blocksContainer.appendChild(container);
				this.activeBlocks.set(block.id, state);
			}

			// Ensure physical DOM order matches visual index order
			if (this.blocksContainer.children[displayIndex] !== container) {
				this.blocksContainer.insertBefore(container, this.blocksContainer.children[displayIndex]);
			}
			displayIndex++;
		}

		// Cleanup orphaned or newly-ignored blocks
		for (const [id, state] of this.activeBlocks.entries()) {
			if (!visibleBlockIds.has(id)) {
				state.container.remove();
				if (state.timer) clearTimeout(state.timer);
				state.markdown?.destroy();
				this.activeBlocks.delete(id);
			}
		}
	}

	private renderTextBlock(
		block: Extract<ContentBlock, { type: "text" }>,
		state: BlockState,
		isGeneratingBlock: boolean,
	) {
		if (state.textCache === block.text) return;

		if (!isGeneratingBlock) {
			if (state.timer) {
				clearTimeout(state.timer);
				state.timer = undefined;
			}
			state.renderSeq++;
			void this.applyMarkdown(state, block.text, true, state.renderSeq);
			return;
		}

		if (state.timer) return;

		state.timer = window.setTimeout(() => {
			state.timer = undefined;
			state.renderSeq++;
			void this.applyMarkdown(state, block.text, false, state.renderSeq);
		}, MARKDOWN_THROTTLE_MS);
	}

	private renderFileBlock(block: Extract<ContentBlock, { type: "file" }>, container: HTMLElement) {
		if (container.hasChildNodes()) return; // Already rendered

		if (block.mimeType.startsWith("image/")) {
			container.appendChild(el("img", "mur-attachment-image", { src: block.data }));
		} else {
			container.appendChild(el("div", "mur-attachment-file-pill", { textContent: `📄 ${block.name || "File"}` }));
		}
	}

	private async applyMarkdown(state: BlockState, content: string, finalize: boolean, seq: number) {
		try {
			const renderer = (state.markdown ??= new StreamingMarkdownRenderer(
				state.container,
				this.config.highlighter,
			));
			await renderer.render(content, finalize);

			if (this.isDestroyed || seq !== state.renderSeq) return;

			state.textCache = content;
		} catch (error) {
			console.error("Failed to render markdown", error);
		}
	}

	private renderError(error: string | null) {
		if (!error) {
			if (this.errorEl) this.errorEl.hidden = true;
			this.cacheError = null;
			return;
		}

		if (!this.errorEl) {
			this.errorEl = el("div", "mur-message-error");
			this.el.appendChild(this.errorEl);
		}

		if (this.cacheError !== error) {
			this.errorEl.textContent = `⚠ ${error}`;
			this.errorEl.hidden = false;
			this.cacheError = error;
		}
	}

	private renderActions(msg: Message, isGenerating: boolean) {
		const shouldShow = msg.blocks.length > 0;

		if (!shouldShow) {
			if (this.actionsEl && this.cacheActionsVisible) {
				this.actionsEl.hidden = true;
				this.cacheActionsVisible = false;
			}
			return;
		}

		if (isGenerating && !this.actionsInitialized) return;

		if (this.actionsInitialized) {
			if (this.actionsEl && !this.cacheActionsVisible) {
				this.actionsEl.hidden = false;
				this.cacheActionsVisible = true;
			}
			return;
		}

		const actionButtons: HTMLElement[] = [];

		for (const plugin of this.config.plugins) {
			let defs: ActionButtonDef[] = [];
			try {
				defs = plugin.getActionButtons?.(msg) ?? [];
			} catch (error) {
				console.error(`Plugin "${plugin.name}" failed during getActionButtons`, error);
			}
			for (const def of defs) {
				actionButtons.push(this.createActionButton(plugin.name, def));
			}
		}

		this.actionsInitialized = true;

		if (actionButtons.length === 0) return;

		this.actionsEl = el("div", "mur-message-actions", null, actionButtons);
		this.el.appendChild(this.actionsEl);
		this.cacheActionsVisible = true;
	}

	private createActionButton(pluginName: string, def: ActionButtonDef): HTMLButtonElement {
		const btn = el("button", "mur-action-icon-btn", {
			title: def.title,
			innerHTML: def.iconHtml,
		});

		btn.dataset.actionId = def.id;
		btn.dataset.pluginName = pluginName;
		btn.addEventListener("click", () => {
			if (!this.currentMsg) return;
			def.onClick({
				message: this.currentMsg,
				buttonEl: btn,
				messageEl: this.el,
				actionId: def.id,
				pluginName,
			});
		});

		return btn;
	}
}
