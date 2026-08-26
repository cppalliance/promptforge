import { el } from "../../utils/dom";
import { ICON_CHEVRON } from "../../utils/icons";
import type { ToolRenderContext, ToolRenderer, ToolStatus } from "./tool-context";
import {
	DEFAULT_MAX_LABEL_CHARS,
	isToolWorking,
	statusText,
	toolArgsText,
	toolLabel,
	toolResultText,
	toolStatus,
} from "./tool-format";

// One preserved activity row in the expanded log: a single-line button with
// a status icon, a human summary, and a chevron that unfolds the call's
// arguments and result.
export interface ToolRow {
	readonly rootEl: HTMLElement;
	readonly toggleEl: HTMLButtonElement;
	readonly iconEl: HTMLElement;
	readonly labelEl: HTMLElement;
	readonly detailsEl: HTMLElement;
	readonly argsPre: HTMLPreElement;
	readonly resultTitleEl: HTMLElement;
	readonly resultPre: HTMLPreElement;
	expanded: boolean;
	label: string;
	status: ToolStatus;
	ctx?: ToolRenderContext;
	renderer?: ToolRenderer;
}

let rowSeq = 0;

export function createToolRow(): ToolRow {
	const detailsId = `mur-tool-row-details-${rowSeq++}`;

	const iconEl = el("span", "mur-tool-row-icon");
	const labelEl = el("span", "mur-tool-row-label");
	const chevronEl = el("span", "mur-tool-row-chevron", { innerHTML: ICON_CHEVRON });
	chevronEl.querySelector("svg")?.setAttribute("aria-hidden", "true");

	const toggleEl = el("button", "mur-tool-row-toggle", { type: "button" }, [iconEl, labelEl, chevronEl]);
	toggleEl.setAttribute("aria-expanded", "false");
	toggleEl.setAttribute("aria-controls", detailsId);

	const argsTitleEl = el("div", "mur-tool-section-title", { textContent: "Arguments" });
	const argsPre = el("pre", "mur-tool-pre");
	const argsSectionEl = el("section", "mur-tool-section", {}, [argsTitleEl, argsPre]);

	const resultTitleEl = el("div", "mur-tool-section-title", { textContent: "Result" });
	const resultPre = el("pre", "mur-tool-pre");
	const resultSectionEl = el("section", "mur-tool-section", {}, [resultTitleEl, resultPre]);

	const detailsEl = el("div", "mur-tool-row-details", {}, [argsSectionEl, resultSectionEl]);
	detailsEl.id = detailsId;
	detailsEl.hidden = true;

	const rootEl = el("div", "mur-tool-row", {}, [toggleEl, detailsEl]);

	const row: ToolRow = {
		rootEl,
		toggleEl,
		iconEl,
		labelEl,
		detailsEl,
		argsPre,
		resultTitleEl,
		resultPre,
		expanded: false,
		label: "",
		status: "pending",
	};

	toggleEl.addEventListener("click", () => {
		setToolRowExpanded(row, !row.expanded);
	});

	return row;
}

export function renderToolRow(
	row: ToolRow,
	ctx: ToolRenderContext,
	renderer: ToolRenderer | undefined,
	maxLabelChars?: number,
): void {
	row.ctx = ctx;
	row.renderer = renderer;
	row.status = toolStatus(ctx);
	row.label = toolLabel(ctx, renderer, maxLabelChars ?? DEFAULT_MAX_LABEL_CHARS);
	row.labelEl.textContent = row.label;
	syncIcon(row);
	row.toggleEl.setAttribute("aria-label", `${row.label} (${statusText(row.status)})`);
	if (row.expanded) syncDetails(row);
}

export function setToolRowExpanded(row: ToolRow, expanded: boolean): void {
	row.expanded = expanded;
	row.toggleEl.setAttribute("aria-expanded", String(expanded));
	row.detailsEl.hidden = !expanded;
	if (expanded) syncDetails(row);
}

function syncIcon(row: ToolRow): void {
	const status = row.status;
	if (isToolWorking(status)) {
		row.iconEl.className = "mur-tool-row-icon mur-tool-row-icon--working";
		row.iconEl.replaceChildren(el("span", "mur-tool-row-spinner"));
	} else {
		row.iconEl.className = `mur-tool-row-icon ${status === "complete" ? "mur-tool-row-icon--done" : "mur-tool-row-icon--error"}`;
		row.iconEl.textContent = status === "complete" ? "✓" : "×";
	}
	row.iconEl.setAttribute("aria-label", statusText(status));
	row.iconEl.title = statusText(status);
}

function syncDetails(row: ToolRow): void {
	const ctx = row.ctx;
	if (!ctx) return;
	row.argsPre.textContent = toolArgsText(ctx, row.renderer);
	row.resultTitleEl.textContent = ctx.toolResult?.isError ? "Error" : "Result";
	row.resultPre.textContent = toolResultText(ctx, row.renderer);
}
