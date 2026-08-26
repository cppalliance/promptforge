import type { BlockRenderContext, ContentBlock, Message } from "../../core/types";

export type ToolCallBlock = Extract<ContentBlock, { type: "tool_call" }>;
export type ToolResultBlock = Extract<ContentBlock, { type: "tool_result" }>;
export type ToolStatus = ToolCallBlock["status"] | "error";

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
	formatArgs?: (ctx: ToolRenderContext) => string | undefined;
	formatResult?: (ctx: ToolRenderContext) => string | undefined;
}

export interface ToolResultCache {
	messages: readonly Message[];
	messageId: string;
	blockId: string;
	toolCallId: string;
	result: ToolResultBlock;
}

const EMPTY_MESSAGE: Message = { id: "", role: "assistant", blocks: [] };

// Resolves the render context for one tool_call block, pairing it with its
// tool_result block (which may live on a later message) and caching that
// pairing against the messages-array identity so repeat renders stay cheap.
export function createToolContext(
	toolCall: ToolCallBlock,
	renderCtx: BlockRenderContext | undefined,
	isGenerating: boolean,
	cache: ToolResultCache | undefined,
): { ctx: ToolRenderContext; cache: ToolResultCache | undefined } {
	const toolResult = resolveToolResult(toolCall, renderCtx, cache);
	const args = parseJson(toolCall.argsText);
	const outputText = toolResult?.outputText ?? "";
	let resultParsed = false;
	let parsedResult: unknown;

	const ctx: ToolRenderContext = {
		toolCall,
		toolResult,
		message: renderCtx?.message ?? EMPTY_MESSAGE,
		messages: renderCtx?.messages ?? [],
		blockIndex: renderCtx?.blockIndex ?? -1,
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

	return { ctx, cache: cacheToolResult(toolCall, renderCtx, toolResult) };
}

function resolveToolResult(
	toolCall: ToolCallBlock,
	renderCtx: BlockRenderContext | undefined,
	cache: ToolResultCache | undefined,
): ToolResultBlock | undefined {
	if (
		cache &&
		renderCtx &&
		cache.messages === renderCtx.messages &&
		cache.messageId === renderCtx.message.id &&
		cache.blockId === toolCall.id &&
		cache.toolCallId === toolCall.toolCallId
	) {
		return cache.result;
	}
	return findToolResult(toolCall.toolCallId, renderCtx);
}

function cacheToolResult(
	toolCall: ToolCallBlock,
	renderCtx: BlockRenderContext | undefined,
	result: ToolResultBlock | undefined,
): ToolResultCache | undefined {
	return result && renderCtx
		? {
				messages: renderCtx.messages,
				messageId: renderCtx.message.id,
				blockId: toolCall.id,
				toolCallId: toolCall.toolCallId,
				result,
			}
		: undefined;
}

function findToolResult(toolCallId: string, renderCtx: BlockRenderContext | undefined): ToolResultBlock | undefined {
	if (!renderCtx) return undefined;

	const messageIndex = renderCtx.messages.findIndex((message) => message.id === renderCtx.message.id);
	const startIndex = messageIndex >= 0 ? messageIndex : 0;

	for (let i = startIndex; i < renderCtx.messages.length; i++) {
		const result = renderCtx.messages[i].blocks.find(
			(block): block is ToolResultBlock => block.type === "tool_result" && block.toolCallId === toolCallId,
		);
		if (result) return result;
	}

	return undefined;
}

export function parseJson(text: string): unknown {
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
