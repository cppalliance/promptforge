import type { ChatPlugin } from "../core/types";
import { IS_TOUCH_DEVICE } from "../utils/device";
import { queryOrThrow } from "../utils/dom";

const MESSAGE_INPUT_LABEL = "Message";
const SEND_BUTTON_LABEL = "Send message";
const STOP_BUTTON_LABEL = "Stop generation";

export interface InputProps {
	container: HTMLElement;
	onSubmit: (text: string) => boolean;
	onStop: () => void;
}

export class Input {
	private form: HTMLFormElement;
	private input: HTMLTextAreaElement;
	private sendBtn: HTMLButtonElement;
	private isGenerating = false;
	private isLoadingSession = false;
	private hasSubmittableText = false;
	private focusTimeout: ReturnType<typeof setTimeout> | null = null;

	private supportsFieldSizing = typeof CSS !== "undefined" && CSS.supports("field-sizing", "content");

	private onInputBound = this.handleInput.bind(this);
	private onKeydownBound = this.handleKeydown.bind(this);
	private onSubmitBound = this.handleFormSubmit.bind(this);

	constructor(
		private props: InputProps,
		private plugins: ChatPlugin[] = [],
	) {
		this.form = queryOrThrow<HTMLFormElement>(this.props.container, ".mur-chat-form");
		this.input = queryOrThrow<HTMLTextAreaElement>(this.props.container, ".mur-chat-input");
		this.sendBtn = queryOrThrow<HTMLButtonElement>(this.props.container, ".mur-send-btn");

		this.ensureInputAccessibleName();

		for (const plugin of plugins) {
			if (plugin.onInputMount) {
				try {
					plugin.onInputMount({
						container: this.props.container,
						form: this.form,
						input: this.input,
						requestSubmitStateSync: () => this.syncSubmitState(),
					});
				} catch (error) {
					console.error(`Plugin "${plugin.name}" failed during onInputMount`, error);
				}
			}
		}

		this.bindEvents();
		this.refreshTextState();
		this.syncSubmitState();
	}

	public focus() {
		this.scheduleFocus();
	}

	public setGeneratingState(isGenerating: boolean, isLoadingSession: boolean) {
		this.isGenerating = isGenerating;
		this.isLoadingSession = isLoadingSession;
		this.sendBtn.classList.toggle("mur-generating", isGenerating);
		this.syncSubmitState();
	}

	public setText(text: string) {
		this.input.value = text;
		if (!this.supportsFieldSizing) {
			this.adjustHeight();
		}
		if (this.refreshTextState()) {
			this.syncSubmitState();
		}
	}

	public getText(): string {
		return this.input.value;
	}

	public destroy() {
		this.clearPendingFocus();
		this.input.removeEventListener("input", this.onInputBound);
		this.input.removeEventListener("keydown", this.onKeydownBound);
		this.form.removeEventListener("submit", this.onSubmitBound);
	}

	private ensureInputAccessibleName() {
		if (this.input.hasAttribute("aria-label") || this.input.hasAttribute("aria-labelledby")) return;
		if (this.input.labels && this.input.labels.length > 0) return;

		this.input.setAttribute("aria-label", MESSAGE_INPUT_LABEL);
	}

	private clearPendingFocus() {
		if (this.focusTimeout === null) return;
		clearTimeout(this.focusTimeout);
		this.focusTimeout = null;
	}

	private scheduleFocus() {
		if (IS_TOUCH_DEVICE) return;

		// Timeout ensures focus works correctly after DOM reflows
		// or when transitioning state (e.g., stopping generation)
		this.clearPendingFocus();
		this.focusTimeout = setTimeout(() => {
			this.focusTimeout = null;
			this.input.focus({ preventScroll: true });
		}, 0);
	}

	private bindEvents() {
		this.input.addEventListener("input", this.onInputBound);
		this.input.addEventListener("keydown", this.onKeydownBound);
		this.form.addEventListener("submit", this.onSubmitBound);
	}

	private handleInput() {
		if (!this.supportsFieldSizing) {
			this.adjustHeight();
		}
		if (this.refreshTextState()) {
			this.syncSubmitState();
		}
	}

	private handleKeydown(e: KeyboardEvent) {
		if (e.key === "Enter" && !e.shiftKey && !e.isComposing && !IS_TOUCH_DEVICE) {
			e.preventDefault();
			this.handleSubmit();
		}
	}

	private handleFormSubmit(e: Event) {
		e.preventDefault();
		this.handleSubmit();
	}

	private adjustHeight() {
		const el = this.input;
		el.style.height = "auto"; // Force synchronous reflow to determine natural height
		const newHeight = Math.min(el.scrollHeight, this.getMaxHeight());
		el.style.height = newHeight + "px";
	}

	private getMaxHeight(): number {
		const maxHeight = Number.parseFloat(window.getComputedStyle(this.input).maxHeight);
		return Number.isFinite(maxHeight) && maxHeight > 0 ? maxHeight : 200;
	}

	private handleSubmit() {
		if (this.isGenerating) {
			this.props.onStop();
			return;
		}

		if (this.isLoadingSession) {
			this.syncSubmitState();
			return;
		}

		const textStateChanged = this.refreshTextState();
		const text = this.input.value;

		if (!this.canSubmit()) {
			if (textStateChanged) {
				this.syncSubmitState();
			}
			return;
		}

		if (!this.props.onSubmit(text)) {
			// Submission rejected, keep text and sync state
			this.syncSubmitState();
			return;
		}

		this.focus();
		this.input.value = "";

		this.refreshTextState();
		if (!this.supportsFieldSizing) {
			this.adjustHeight();
		}

		this.syncSubmitState();
	}

	private syncSubmitState() {
		const buttonLabel = this.isGenerating ? STOP_BUTTON_LABEL : SEND_BUTTON_LABEL;
		this.sendBtn.setAttribute("aria-label", buttonLabel);
		this.sendBtn.title = buttonLabel;

		if (this.isGenerating) {
			this.sendBtn.disabled = false;
			return;
		}

		this.sendBtn.disabled = !this.canSubmit();
	}

	private canSubmit(): boolean {
		return (
			!this.isLoadingSession && !this.isSubmitBlocked() && (this.hasSubmittableText || this.hasPendingPluginData())
		);
	}

	private isSubmitBlocked(): boolean {
		return this.plugins.some((p) => {
			try {
				return Boolean(p.isSubmitBlocked?.());
			} catch (error) {
				console.error(`Plugin "${p.name}" failed during isSubmitBlocked`, error);
				return false;
			}
		});
	}

	private hasPendingPluginData(): boolean {
		return this.plugins.some((p) => {
			try {
				return Boolean(p.hasPendingData?.());
			} catch (error) {
				console.error(`Plugin "${p.name}" failed during hasPendingData`, error);
				return false;
			}
		});
	}

	private refreshTextState(): boolean {
		const hasSubmittableText = /\S/.test(this.input.value);
		if (hasSubmittableText === this.hasSubmittableText) return false;

		this.hasSubmittableText = hasSubmittableText;
		return true;
	}
}
