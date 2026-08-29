import { extractPlainText } from "../core/msg-utils";
import type { Message } from "../core/types";
import { formatDuration, formatRelativeTime, HOUR_MS, MINUTE_MS } from "../utils/format";
import { ICON_CHECK, ICON_COPY, ICON_FORK } from "../utils/icons";

const COPY_FEEDBACK_MS = 2000;

/**
 * Quiet action row mounted once per completed model turn, below the final
 * visible message and outside any collapsible thinking/tool activity.
 */
export class TurnFooter {
	public readonly el: HTMLElement;

	private readonly copyButton: HTMLButtonElement;
	private readonly forkButton: HTMLButtonElement;
	private readonly stampEl: HTMLSpanElement;
	private readonly timeEl: HTMLTimeElement;
	private readonly tooltipEl: HTMLSpanElement;

	private message: Message;
	private durationMs?: number;
	private refreshTimer?: number;
	private copyTimer?: number;
	private destroyed = false;

	constructor(message: Message, durationMs?: number) {
		this.message = message;
		this.durationMs = durationMs;

		this.el = document.createElement("div");
		this.el.className = "mur-turn-footer";

		this.copyButton = document.createElement("button");
		this.copyButton.type = "button";
		this.copyButton.className = "mur-turn-footer-button";
		this.copyButton.innerHTML = ICON_COPY;
		this.copyButton.setAttribute("aria-label", "Copy response");
		this.copyButton.title = "Copy response";
		this.copyButton.addEventListener("click", () => {
			void this.handleCopy();
		});

		// Intentionally inert: visual placeholder for a future fork action.
		this.forkButton = document.createElement("button");
		this.forkButton.type = "button";
		this.forkButton.className = "mur-turn-footer-button";
		this.forkButton.innerHTML = ICON_FORK;
		this.forkButton.setAttribute("aria-label", "Fork conversation");
		this.forkButton.title = "Fork conversation";

		this.stampEl = document.createElement("span");
		this.stampEl.className = "mur-turn-footer-stamp";
		this.stampEl.tabIndex = 0;

		this.timeEl = document.createElement("time");
		this.tooltipEl = document.createElement("span");
		this.tooltipEl.className = "mur-turn-footer-tooltip";
		this.tooltipEl.setAttribute("role", "tooltip");
		this.stampEl.append(this.timeEl, this.tooltipEl);

		this.el.append(this.copyButton, this.forkButton, this.stampEl);
		this.renderTimestamp();
	}

	public update(message: Message, durationMs?: number): void {
		this.message = message;
		this.durationMs = durationMs;
		this.renderTimestamp();
	}

	public destroy(): void {
		this.destroyed = true;
		if (this.refreshTimer !== undefined) window.clearTimeout(this.refreshTimer);
		if (this.copyTimer !== undefined) window.clearTimeout(this.copyTimer);
		this.refreshTimer = undefined;
		this.copyTimer = undefined;
		this.el.remove();
	}

	private async handleCopy(): Promise<void> {
		if (this.destroyed) return;
		if (typeof navigator === "undefined" || !navigator.clipboard) return;
		try {
			await navigator.clipboard.writeText(extractPlainText(this.message));
		} catch {
			// Clipboard denial is non-fatal: the checkmark feedback simply does not appear.
			return;
		}
		if (this.destroyed) return;
		this.copyButton.innerHTML = ICON_CHECK;
		this.copyButton.classList.add("mur-turn-footer-button--copied");
		if (this.copyTimer !== undefined) window.clearTimeout(this.copyTimer);
		this.copyTimer = window.setTimeout(() => {
			this.copyButton.innerHTML = ICON_COPY;
			this.copyButton.classList.remove("mur-turn-footer-button--copied");
		}, COPY_FEEDBACK_MS);
	}

	private renderTimestamp(): void {
		if (this.refreshTimer !== undefined) {
			window.clearTimeout(this.refreshTimer);
			this.refreshTimer = undefined;
		}

		const timestamp = this.message.updatedAt ?? this.message.createdAt;
		if (timestamp === undefined || !Number.isFinite(timestamp)) {
			this.stampEl.hidden = true;
			return;
		}
		this.stampEl.hidden = false;

		const date = new Date(timestamp);
		const elapsedMs = Math.max(0, Date.now() - timestamp);
		this.timeEl.dateTime = date.toISOString();
		this.timeEl.textContent = formatRelativeTime(elapsedMs);

		this.tooltipEl.textContent = "";
		const absoluteEl = document.createElement("span");
		absoluteEl.className = "mur-turn-footer-tooltip-time";
		absoluteEl.textContent = date.toLocaleString();
		this.tooltipEl.appendChild(absoluteEl);

		if (this.durationMs !== undefined && this.durationMs > 0) {
			const durationEl = document.createElement("span");
			durationEl.className = "mur-turn-footer-tooltip-duration";
			durationEl.textContent = `Worked for ${formatDuration(this.durationMs)}`;
			this.tooltipEl.appendChild(durationEl);
		}

		this.scheduleRefresh(elapsedMs);
	}

	// Re-render exactly when the displayed relative value can roll over
	// (next minute or hour boundary of the elapsed time).
	private scheduleRefresh(elapsedMs: number): void {
		const unit = elapsedMs < HOUR_MS ? MINUTE_MS : HOUR_MS;
		const delay = unit - (elapsedMs % unit) + 25;
		this.refreshTimer = window.setTimeout(() => {
			if (!this.destroyed) this.renderTimestamp();
		}, delay);
	}
}
