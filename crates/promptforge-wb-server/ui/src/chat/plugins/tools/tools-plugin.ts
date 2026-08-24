import "./tools.css";
import type { BlockRenderContext, ChatPlugin, ContentBlock, Message } from "../../core/types";
import { el } from "../../utils/dom";
import { ICON_CHEVRON } from "../../utils/icons";

type ToolCallBlock = Extract<ContentBlock, { type: "tool_call" }>;
type ToolResultBlock = Extract<ContentBlock, { type: "tool_result" }>;

export interface ToolRenderContext {
	toolCall: ToolCallBlock;
	toolResult?: ToolResultBlock;
	message: Message;
	messages: readonly Message[];
	blockIndex: number;
	isGenerating: boolean;
	args: unknown;
	argsText: string;
	result: unknown;
	outputText: string;
}

export interface ToolRenderer {
	label?: string | ((ctx: ToolRenderContext) => string | undefined);
	preview?: (ctx: ToolRenderContext) => string | undefined;
	formatArgs?: (ctx: ToolRenderContext) => string | undefined;
	formatResult?: (ctx: ToolRenderContext) => string | undefined;
}

export interface ToolsPluginConfig {
	defaultExpanded?: boolean | ((ctx: ToolRenderContext) => boolean);
	maxLabelChars?: number;
	maxPreviewChars?: number;
	tools?: Record<string, ToolRenderer>;
}

interface ToolState {
	expanded: boolean;
	rootEl: HTMLElement;
	ctx?: ToolRenderContext;
	renderer?: ToolRenderer;
	resultCache?: ToolResultCache;
	previewText: string;
	buttonEl: HTMLButtonElement;
	titleEl: HTMLElement;
	statusEl: HTMLElement;
	previewEl?: HTMLElement;
	detailsEl?: HTMLElement;
	details?: ToolDetailsState;
}

interface ToolDetailsState {
	argsPre: HTMLPreElement;
	resultSectionEl: HTMLElement;
	resultTitleEl: HTMLElement;
	resultPre: HTMLPreElement;
}

interface ToolResultCache {
	messages: readonly Message[];
	messageId: string;
	blockId: string;
	toolCallId: string;
	result: ToolResultBlock;
}

const DEFAULT_MAX_LABEL_CHARS = 120;
const DEFAULT_MAX_PREVIEW_CHARS = 240;
const MAX_ARG_SUMMARY_VALUE_CHARS = 40;
const EMPTY_MESSAGE: Message = { id: "", role: "assistant", blocks: [] };

export function ToolsPlugin(config: ToolsPluginConfig = {}): ChatPlugin {
	const stateMap = new WeakMap<HTMLElement, ToolState>();

	return {
		name: "tools",
		onBlockRender: (block, containerEl, isGenerating, renderCtx) => {
			if (block.type !== "tool_call") return false;

			let state = stateMap.get(containerEl);
			const ctx = createToolContext(block, renderCtx, isGenerating, state);
			const renderer = config.tools?.[block.name];

			if (!state) {
				state = createToolState(containerEl, resolveDefaultExpanded(config.defaultExpanded, ctx));
				containerEl.replaceChildren(state.buttonEl);
				state.buttonEl.addEventListener("click", () => {
					state!.expanded = !state!.expanded;
					syncExpansion(state!);
				});
				stateMap.set(containerEl, state);
			}
			cacheToolResult(state, block, renderCtx, ctx.toolResult);

			renderTool(containerEl, state, ctx, renderer, config);
			return true;
		},
	};
}

function createToolState(rootEl: HTMLElement, expanded: boolean): ToolState {
	const chevronEl = el("span", "mur-tool-chevron", { innerHTML: ICON_CHEVRON });
	const titleEl = el("span", "mur-tool-title");
	const statusEl = el("span", "mur-tool-status");
	const buttonEl = el("button", "mur-tool-summary", { type: "button" }, [statusEl, titleEl, chevronEl]);

	const state = {
		expanded,
		rootEl,
		previewText: "",
		buttonEl,
		titleEl,
		statusEl,
	};

	syncExpansion(state);
	return state;
}

function renderTool(
	containerEl: HTMLElement,
	state: ToolState,
	ctx: ToolRenderContext,
	renderer: ToolRenderer | undefined,
	config: ToolsPluginConfig,
): void {
	const status = ctx.toolResult?.isError ? "error" : ctx.toolCall.status;
	containerEl.className = `mur-content-block mur-block-tool_call mur-tool mur-tool-${status}`;

	const label = rendererLabel(renderer, ctx) ?? defaultToolLabel(ctx.toolCall, ctx.args);
	const preview = renderer?.preview?.(ctx) ?? defaultPreview(ctx);
	const statusText = statusLabel(status);

	state.ctx = ctx;
	state.renderer = renderer;
	state.titleEl.textContent = truncateText(label, config.maxLabelChars ?? DEFAULT_MAX_LABEL_CHARS);
	state.statusEl.textContent = statusSymbol(status);
	state.statusEl.title = statusText;
	state.statusEl.setAttribute("aria-label", statusText);
	state.buttonEl.setAttribute("aria-label", `${label} (${statusText})`);

	state.previewText = truncateText(preview ?? "", config.maxPreviewChars ?? DEFAULT_MAX_PREVIEW_CHARS);

	syncExpansion(state);
}

function createToolContext(
	toolCall: ToolCallBlock,
	ctx: BlockRenderContext | undefined,
	isGenerating: boolean,
	state: ToolState | undefined,
): ToolRenderContext {
	const messages = ctx?.messages ?? [];
	const toolResult = resolveToolResult(toolCall, ctx, state);
	const args = parseJson(toolCall.argsText);
	const outputText = toolResult?.outputText ?? "";
	let resultParsed = false;
	let parsedResult: unknown;

	return {
		toolCall,
		toolResult,
		message: ctx?.message ?? EMPTY_MESSAGE,
		messages,
		blockIndex: ctx?.blockIndex ?? -1,
		isGenerating,
		args,
		argsText: toolCall.argsText,
		outputText,
		get result() {
			if (!resultParsed) {
				parsedResult = parseJson(outputText);
				resultParsed = true;
			}
			return parsedResult;
		},
	};
}

function resolveToolResult(
	toolCall: ToolCallBlock,
	ctx: BlockRenderContext | undefined,
	state: ToolState | undefined,
): ToolResultBlock | undefined {
	const cached = state?.resultCache;
	if (
		cached &&
		ctx &&
		cached.messages === ctx.messages &&
		cached.messageId === ctx.message.id &&
		cached.blockId === toolCall.id &&
		cached.toolCallId === toolCall.toolCallId
	) {
		return cached.result;
	}

	const result = findToolResult(toolCall.toolCallId, ctx);
	if (state) cacheToolResult(state, toolCall, ctx, result);
	return result;
}

function cacheToolResult(
	state: ToolState,
	toolCall: ToolCallBlock,
	ctx: BlockRenderContext | undefined,
	result: ToolResultBlock | undefined,
): void {
	state.resultCache =
		result && ctx
			? {
					messages: ctx.messages,
					messageId: ctx.message.id,
					blockId: toolCall.id,
					toolCallId: toolCall.toolCallId,
					result,
				}
			: undefined;
}

function findToolResult(toolCallId: string, ctx: BlockRenderContext | undefined): ToolResultBlock | undefined {
	if (!ctx) return undefined;

	const messageIndex = ctx.messages.findIndex((message) => message.id === ctx.message.id);
	const startIndex = messageIndex >= 0 ? messageIndex : 0;

	for (let i = startIndex; i < ctx.messages.length; i++) {
		const result = ctx.messages[i].blocks.find(
			(block): block is ToolResultBlock => block.type === "tool_result" && block.toolCallId === toolCallId,
		);
		if (result) return result;
	}

	return undefined;
}

function rendererLabel(renderer: ToolRenderer | undefined, ctx: ToolRenderContext): string | undefined {
	if (!renderer?.label) return undefined;
	return typeof renderer.label === "function" ? renderer.label(ctx) : renderer.label;
}

function resolveDefaultExpanded(
	defaultExpanded: ToolsPluginConfig["defaultExpanded"],
	ctx: ToolRenderContext,
): boolean {
	if (typeof defaultExpanded === "function") return defaultExpanded(ctx);
	return defaultExpanded ?? false;
}

function syncExpansion(state: ToolState): void {
	state.buttonEl.setAttribute("aria-expanded", String(state.expanded));
	syncPreview(state);

	if (state.expanded && state.ctx) {
		renderDetails(state);
		return;
	}

	clearDetails(state);
}

function renderDetails(state: ToolState): void {
	const ctx = state.ctx;
	if (!ctx) return;
	const detailsEl = ensureDetailsEl(state);
	const details = ensureDetails(state);

	detailsEl.hidden = false;
	details.argsPre.textContent = state.renderer?.formatArgs?.(ctx) ?? defaultArgsText(ctx);
	details.resultTitleEl.textContent = ctx.toolResult?.isError ? "Error" : "Result";
	details.resultPre.textContent = state.renderer?.formatResult?.(ctx) ?? defaultResultText(ctx);
	details.resultSectionEl.hidden = false;
}

function clearDetails(state: ToolState): void {
	if (state.detailsEl) {
		state.detailsEl.remove();
		state.detailsEl = undefined;
	}
	state.details = undefined;
}

function ensureDetails(state: ToolState): ToolDetailsState {
	if (state.details) return state.details;

	const argsTitleEl = el("div", "mur-tool-section-title", { textContent: "Arguments" });
	const argsPre = el("pre", "mur-tool-pre");
	const argsSectionEl = el("section", "mur-tool-section", {}, [argsTitleEl, argsPre]);

	const resultTitleEl = el("div", "mur-tool-section-title", { textContent: "Result" });
	const resultPre = el("pre", "mur-tool-pre");
	const resultSectionEl = el("section", "mur-tool-section", {}, [resultTitleEl, resultPre]);

	ensureDetailsEl(state).replaceChildren(argsSectionEl, resultSectionEl);
	state.details = {
		argsPre,
		resultSectionEl,
		resultTitleEl,
		resultPre,
	};
	return state.details;
}

function syncPreview(state: ToolState): void {
	if (!state.previewText || state.expanded) {
		state.previewEl?.remove();
		state.previewEl = undefined;
		return;
	}

	const previewEl = ensurePreviewEl(state);
	previewEl.textContent = state.previewText;
}

function ensurePreviewEl(state: ToolState): HTMLElement {
	if (state.previewEl) return state.previewEl;

	const previewEl = el("div", "mur-tool-preview");
	state.rootEl.insertBefore(previewEl, state.detailsEl ?? null);
	state.previewEl = previewEl;
	return previewEl;
}

function ensureDetailsEl(state: ToolState): HTMLElement {
	if (state.detailsEl) return state.detailsEl;

	const detailsEl = el("div", "mur-tool-details");
	state.rootEl.appendChild(detailsEl);
	state.detailsEl = detailsEl;
	return detailsEl;
}

function defaultToolLabel(toolCall: ToolCallBlock, args: unknown): string {
	const name = toolCall.name || "tool";
	const summary = summarizeArgs(args, toolCall.argsText);
	return summary ? `${name} ${summary}` : name;
}

function summarizeArgs(args: unknown, argsText: string): string {
	if (args && typeof args === "object" && !Array.isArray(args)) {
		const entries = Object.entries(args as Record<string, unknown>).filter(
			([, value]) => value !== undefined && value !== null,
		);
		if (entries.length === 0) return "";

		const preferred = [
			"command",
			"cmd",
			"pattern",
			"query",
			"path",
			"dir_path",
			"file",
			"filePath",
			"filepath",
			"url",
			"name",
		];
		const preferredEntries: Array<[string, unknown]> = [];
		for (const key of preferred) {
			const match = entries.find(([entryKey]) => entryKey === key);
			if (match) preferredEntries.push(match);
			if (preferredEntries.length >= 2) break;
		}

		const summaryEntries = preferredEntries.length > 0 ? preferredEntries : entries.slice(0, 2);
		if (summaryEntries.length > 0) {
			if (summaryEntries.length === 1 && preferredEntries.length === 1) {
				return compactValue(summaryEntries[0][1]);
			}
			return summaryEntries.map(([key, value]) => `${key}=${compactValue(value)}`).join(" ");
		}

		return `${entries.length} args`;
	}

	if (Array.isArray(args)) return `${args.length} items`;
	if (args !== undefined) return compactValue(args);

	const raw = argsText.trim().replace(/\s+/g, " ");
	return raw === "{}" ? "" : raw;
}

function compactValue(value: unknown): string {
	const text =
		typeof value === "string"
			? value
			: typeof value === "number" || typeof value === "boolean" || value === null
				? String(value)
				: JSON.stringify(value);
	return truncateText(text.replace(/\s+/g, " "), MAX_ARG_SUMMARY_VALUE_CHARS);
}

function defaultPreview(ctx: ToolRenderContext): string | undefined {
	if (!ctx.toolResult?.isError) return undefined;
	return ctx.outputText || "Tool failed.";
}

function defaultArgsText(ctx: ToolRenderContext): string {
	if (ctx.args !== undefined) return JSON.stringify(ctx.args, null, 2);
	return ctx.argsText.trim() || "{}";
}

function defaultResultText(ctx: ToolRenderContext): string {
	if (!ctx.toolResult) {
		if (ctx.toolCall.status === "running") return "Running...";
		if (ctx.toolCall.status === "pending") return "Waiting for result...";
		return "No result.";
	}

	if (ctx.result !== undefined) return JSON.stringify(ctx.result, null, 2);
	return ctx.outputText;
}

function parseJson(text: string): unknown {
	const firstChar = firstNonWhitespaceChar(text);
	if (!firstChar || !'{["-0123456789tfn'.includes(firstChar)) return undefined;

	try {
		return JSON.parse(text);
	} catch {
		return undefined;
	}
}

function firstNonWhitespaceChar(text: string): string {
	for (let i = 0; i < text.length; i++) {
		const char = text[i];
		if (char !== " " && char !== "\n" && char !== "\r" && char !== "\t") return char;
	}
	return "";
}

function statusSymbol(status: ToolCallBlock["status"] | "error"): string {
	switch (status) {
		case "complete":
			return "✓";
		case "error":
			return "×";
		default:
			return "...";
	}
}

function statusLabel(status: ToolCallBlock["status"] | "error"): string {
	return status;
}

function truncateText(text: string, maxChars: number): string {
	if (text.length <= maxChars) return text;
	if (maxChars <= 3) return text.slice(0, maxChars);
	return `${text.slice(0, maxChars - 3)}...`;
}
