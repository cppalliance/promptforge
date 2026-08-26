import type { ToolCallBlock, ToolRenderContext, ToolRenderer, ToolStatus } from "./tool-context";

export const DEFAULT_MAX_LABEL_CHARS = 120;
const MAX_ARG_SUMMARY_VALUE_CHARS = 40;

export function toolStatus(ctx: ToolRenderContext): ToolStatus {
	return ctx.toolResult?.isError ? "error" : ctx.toolCall.status;
}

export function isToolWorking(status: ToolStatus): boolean {
	return status === "streaming" || status === "pending" || status === "running";
}

export function statusText(status: ToolStatus): string {
	switch (status) {
		case "complete":
			return "complete";
		case "error":
			return "error";
		case "streaming":
			return "receiving";
		case "pending":
			return "pending";
		case "running":
			return "running";
	}
}

export function toolLabel(ctx: ToolRenderContext, renderer: ToolRenderer | undefined, maxChars: number): string {
	const label = rendererLabel(renderer, ctx) ?? defaultToolLabel(ctx.toolCall, ctx.args);
	return truncateText(label, maxChars);
}

export function toolArgsText(ctx: ToolRenderContext, renderer: ToolRenderer | undefined): string {
	return renderer?.formatArgs?.(ctx) ?? defaultArgsText(ctx);
}

export function toolResultText(ctx: ToolRenderContext, renderer: ToolRenderer | undefined): string {
	return renderer?.formatResult?.(ctx) ?? defaultResultText(ctx);
}

function rendererLabel(renderer: ToolRenderer | undefined, ctx: ToolRenderContext): string | undefined {
	if (!renderer?.label) return undefined;
	return typeof renderer.label === "function" ? renderer.label(ctx) : renderer.label;
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

export function truncateText(text: string, maxChars: number): string {
	if (text.length <= maxChars) return text;
	if (maxChars <= 3) return text.slice(0, maxChars);
	return `${text.slice(0, maxChars - 3)}...`;
}
