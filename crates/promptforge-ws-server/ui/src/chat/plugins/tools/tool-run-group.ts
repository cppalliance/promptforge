import { el } from "../../utils/dom";
import { ICON_CHEVRON } from "../../utils/icons";

const LINE_ANIM_MS = 180;
const LOG_BOTTOM_TOLERANCE_PX = 8;

let groupSeq = 0;

// The collapsible block that hosts one agent run's tool activity. Collapsed
// (the default), the header shows a one-line window: each new activity line
// scrolls the previous one up and out at constant height. Expanded, the
// preserved log of rows shows below, pinned to the newest row until the
// user scrolls up inside it.
export class ToolRunGroup {
	public readonly rootEl: HTMLElement;
	private readonly toggleEl: HTMLButtonElement;
	private readonly windowEl: HTMLElement;
	private readonly logEl: HTMLElement;
	private readonly liveEl: HTMLElement;
	private lineEl: HTMLElement | null = null;
	private currentLineText = "";
	private expanded: boolean;
	private autoPin = true;
	private animTimer: number | undefined;

	constructor(onToggle: () => void, expanded: boolean) {
		const logId = `mur-tool-run-log-${groupSeq++}`;

		const chevronEl = el("span", "mur-tool-run-chevron", { innerHTML: ICON_CHEVRON });
		chevronEl.querySelector("svg")?.setAttribute("aria-hidden", "true");

		this.windowEl = el("span", "mur-tool-run-window");
		this.toggleEl = el("button", "mur-tool-run-toggle", { type: "button" }, [chevronEl, this.windowEl]);
		this.toggleEl.setAttribute("aria-controls", logId);
		this.toggleEl.addEventListener("click", onToggle);

		this.logEl = el("div", "mur-tool-run-log");
		this.logEl.id = logId;

		this.liveEl = el("span", "mur-tool-run-sr-only");
		this.liveEl.setAttribute("aria-live", "polite");

		this.rootEl = el("div", "mur-tool-run", {}, [this.toggleEl, this.logEl, this.liveEl]);

		this.logEl.addEventListener("scroll", () => {
			const distanceFromBottom = this.logEl.scrollHeight - this.logEl.clientHeight - this.logEl.scrollTop;
			this.autoPin = distanceFromBottom <= LOG_BOTTOM_TOLERANCE_PX;
		});

		this.expanded = expanded;
		this.syncExpanded();
	}

	public get lineText(): string {
		return this.currentLineText;
	}

	public isExpanded(): boolean {
		return this.expanded;
	}

	public setExpanded(expanded: boolean): void {
		this.expanded = expanded;
		this.syncExpanded();
		if (expanded) {
			this.autoPin = true;
			this.pinLogToBottom();
		}
	}

	public appendRow(rowEl: HTMLElement): void {
		// Rows join in block order; a row already in the log keeps its place.
		if (rowEl.parentElement !== this.logEl) {
			this.logEl.appendChild(rowEl);
		}
		this.maybePin();
	}

	// Render-time pinning, mirroring the thinking preview: while expanded the
	// log tracks the newest row unless the user has scrolled up inside it.
	public maybePin(): void {
		if (this.expanded && this.autoPin) this.pinLogToBottom();
	}

	public pushLine(text: string): void {
		if (text === this.currentLineText) return;
		this.currentLineText = text;
		this.toggleEl.setAttribute("aria-label", `Tool activity: ${text}`);

		this.finishAnimation();
		const prev = this.lineEl;
		const next = el("span", "mur-tool-run-line", { textContent: text });
		this.lineEl = next;

		if (!prev || prefersReducedMotion()) {
			prev?.remove();
			this.windowEl.replaceChildren(next);
			return;
		}

		// The previous line scrolls up and out while the new one rises in;
		// only transform animates, so the window height never changes.
		this.windowEl.appendChild(next);
		prev.classList.add("mur-tool-run-line--exit");
		next.classList.add("mur-tool-run-line--enter");
		void next.offsetWidth;
		prev.classList.add("mur-tool-run-line--go");
		next.classList.add("mur-tool-run-line--go");
		this.animTimer = window.setTimeout(() => {
			this.animTimer = undefined;
			prev.remove();
			next.classList.remove("mur-tool-run-line--enter", "mur-tool-run-line--go");
		}, LINE_ANIM_MS);
	}

	// Announces a state transition (the resting summary) without ever
	// narrating per-activity text.
	public announce(text: string): void {
		this.liveEl.textContent = text;
	}

	public destroy(): void {
		this.finishAnimation();
	}

	private syncExpanded(): void {
		this.toggleEl.setAttribute("aria-expanded", String(this.expanded));
		this.logEl.hidden = !this.expanded;
	}

	private pinLogToBottom(): void {
		this.logEl.scrollTop = this.logEl.scrollHeight;
	}

	private finishAnimation(): void {
		if (this.animTimer === undefined) return;
		window.clearTimeout(this.animTimer);
		this.animTimer = undefined;
		const lines = this.windowEl.querySelectorAll(".mur-tool-run-line");
		lines.forEach((line, index) => {
			if (index < lines.length - 1) {
				line.remove();
			} else {
				line.classList.remove("mur-tool-run-line--enter", "mur-tool-run-line--exit", "mur-tool-run-line--go");
			}
		});
	}
}

function prefersReducedMotion(): boolean {
	return (
		typeof window !== "undefined" &&
		typeof window.matchMedia === "function" &&
		window.matchMedia("(prefers-reduced-motion: reduce)").matches
	);
}
