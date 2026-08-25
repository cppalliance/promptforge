import { parseSSE } from "../../utils/sse";
import { uuidv7 } from "../../utils/uuid";
import type { ChatProvider, ChatRequest, FinishReason, Message, StreamEvent } from "../types";

type OpenAIStreamDelta = {
	content?: string | null;
	tool_calls?: Array<{
		index: number;
		id?: string;
		type?: string;
		function?: {
			name?: string;
			arguments?: string;
		};
	}>;
	reasoning?: string | { encrypted?: string };
	reasoning_encrypted?: string;
	reasoning_content?: string;
	reasoning_text?: string;
	[key: string]: unknown;
};

interface OpenAIStreamChunk {
	id?: string;
	choices?: Array<{
		delta?: OpenAIStreamDelta;
		finish_reason?: string;
	}>;
	usage?: {
		prompt_tokens?: number;
		completion_tokens?: number;
		total_tokens?: number;
		prompt_tokens_details?: {
			cached_tokens?: number;
		};
	};
}

type OpenAIContentPart = { type: "text"; text: string } | { type: "image_url"; image_url: { url: string } };

const REASONING_FIELDS = ["reasoning_content", "reasoning", "reasoning_text"] as const;
const DEFAULT_TITLE_SYSTEM_PROMPT =
	"You generate concise chat titles. Reply only with the title, without quotes or extra text.";

export class OpenAIProvider implements ChatProvider {
	constructor(
		private apiKey: string,
		private endpoint: string,
		private model: string,
	) {}

	async streamChat(request: ChatRequest, onEvent: (event: StreamEvent) => void): Promise<void> {
		const { model = this.model, ...restOptions } = request.options;

		const response = await fetch(this.endpoint, {
			method: "POST",
			headers: this.headers(),
			body: JSON.stringify({
				...restOptions,
				model,
				messages: this.formatMessagesWithInstructions(request.messages, request.instructions),
				stream: true,
				...(request.tools ? { tools: request.tools } : {}),
				stream_options: {
					include_usage: true,
					...((restOptions.stream_options as object) || {}),
				},
			}),
			signal: request.signal,
		});

		if (!response.ok) {
			const errorMsg = await this.extractErrorMessage(response);
			throw new Error(`API Error ${response.status}: ${errorMsg}`);
		}

		let messageStarted = false;
		let currentMessageId = uuidv7();
		let currentTextBlockId: string | null = null;
		let currentReasoningBlockId: string | null = null;

		// Map OpenAI's tool call index to our block IDs
		const activeToolCalls = new Map<number, string>();

		let finishEmitted = false;

		await parseSSE(response, (data) => {
			if (data === "[DONE]") return true;

			// Flat try/catch: Just parse and exit early if it's a broken chunk
			let parsed: OpenAIStreamChunk;
			try {
				parsed = JSON.parse(data);
			} catch {
				return; // Ignore partial/broken JSON payload
			}
			if (parsed.usage) {
				const input = parsed.usage.prompt_tokens ?? 0;
				const output = parsed.usage.completion_tokens ?? 0;
				onEvent({
					type: "usage",
					input,
					output,
					total: parsed.usage.total_tokens ?? input + output,
					cacheRead: parsed.usage.prompt_tokens_details?.cached_tokens ?? 0,
				});
			}

			const choice = parsed.choices?.[0];
			if (!choice) return;

			// 1. Emit start event on first chunk
			if (!messageStarted) {
				currentMessageId = parsed.id || currentMessageId;
				onEvent({
					type: "message_start",
					message: { id: currentMessageId, role: "assistant", blocks: [] },
				});
				messageStarted = true;
			}

			const delta: OpenAIStreamDelta = choice.delta ?? {};

			// 2. Handle Reasoning
			const reasoningData = this.extractReasoning(delta);
			if (reasoningData) {
				if (!currentReasoningBlockId) currentReasoningBlockId = uuidv7();
				currentTextBlockId = null;

				onEvent({
					type: "reasoning_delta",
					messageId: currentMessageId,
					blockId: currentReasoningBlockId,
					delta: reasoningData.text,
					encrypted: reasoningData.encrypted,
				});
			}

			// 3. Handle Text Content
			if (delta.content) {
				if (!currentTextBlockId) currentTextBlockId = uuidv7();
				onEvent({
					type: "text_delta",
					messageId: currentMessageId,
					blockId: currentTextBlockId,
					delta: delta.content,
				});
			}

			// 4. Handle Tool Calls
			if (delta.tool_calls && Array.isArray(delta.tool_calls)) {
				for (const tc of delta.tool_calls) {
					const index = tc.index;
					// If it has an ID, it's a new tool call
					if (tc.id) {
						currentTextBlockId = null;

						const blockId = uuidv7();
						activeToolCalls.set(index, blockId);
						onEvent({
							type: "tool_call_start",
							messageId: currentMessageId,
							block: {
								id: blockId,
								type: "tool_call",
								toolCallId: tc.id,
								name: tc.function?.name || "",
								argsText: tc.function?.arguments || "",
								status: "streaming",
							},
						});
					}
					// Otherwise, it's appending arguments to an existing tool call
					else if (activeToolCalls.has(index)) {
						onEvent({
							type: "tool_call_delta",
							messageId: currentMessageId,
							blockId: activeToolCalls.get(index)!,
							name: tc.function?.name,
							argsDelta: tc.function?.arguments || "",
						});
					}
				}
			}

			// 5. Handle Finish Reason
			if (choice.finish_reason) {
				if (choice.finish_reason === "content_filter") {
					throw new Error("Generation stopped by provider content filter.");
				}
				if (choice.finish_reason === "network_error") {
					throw new Error("Generation stopped due to a provider network error.");
				}

				const reasonMap: Record<string, FinishReason> = {
					stop: "stop",
					length: "length",
					tool_calls: "tool_use",
				};
				onEvent({
					type: "finish",
					reason: reasonMap[choice.finish_reason] || "stop",
				});
				finishEmitted = true;
			}
			return undefined;
		});

		// If it finishes normally but didn't emit a finish reason (some providers do this)
		if (!finishEmitted) {
			onEvent({ type: "finish", reason: "stop" });
		}
	}

	private async extractErrorMessage(response: Response): Promise<string> {
		const text = await response.text();
		try {
			const parsed = JSON.parse(text);
			return parsed.error?.message || parsed.message || parsed.error?.metadata?.raw || text;
		} catch {
			return text;
		}
	}

	async generateTitle(request: ChatRequest): Promise<string> {
		try {
			const { model = this.model, stream_options: _streamOptions, ...restOptions } = request.options;
			const titleSystemPrompt =
				typeof request.instructions === "string" && request.instructions.trim().length > 0
					? request.instructions
					: DEFAULT_TITLE_SYSTEM_PROMPT;

			let endIndex = request.messages.findIndex((m) => m.role === "assistant" && m.blocks.length > 0);
			if (endIndex === -1) endIndex = Math.min(request.messages.length - 1, 3);

			const contextMessages = request.messages.slice(0, endIndex + 1);
			const formattedMessages = [
				{ role: "system", content: titleSystemPrompt },
				...this.formatMessages(contextMessages),
				{
					role: "user",
					content:
						"Summarize the above conversation in 3-5 words. Reply ONLY with the title, no quotes, no extra text.",
				},
			];

			const response = await fetch(this.endpoint, {
				method: "POST",
				headers: this.headers(),
				body: JSON.stringify({
					...restOptions,
					model,
					messages: formattedMessages,
					stream: false,
				}),
				signal: request.signal,
			});

			if (!response.ok) return "";
			const data = await response.json();
			return this.normalizeTitle(data.choices[0]?.message?.content);
		} catch (error) {
			const isAbort = error instanceof Error && error.name === "AbortError";
			if (!isAbort && !request.signal.aborted) {
				console.warn("Failed to generate chat title.", error);
			}
			return "";
		}
	}

	private normalizeTitle(title: unknown): string {
		if (typeof title !== "string") return "";

		const normalized = title.replace(/\s+/g, " ").trim();
		const unquoted = normalized.replace(/^['"]+|['"]+$/g, "").trim();

		if (unquoted.length <= 80) return unquoted;
		return `${unquoted.slice(0, 77).trimEnd()}...`;
	}

	private headers(): Record<string, string> {
		const headers: Record<string, string> = {
			"Content-Type": "application/json",
		};
		const apiKey = this.apiKey.trim();
		if (apiKey) {
			headers.Authorization = `Bearer ${apiKey}`;
		}
		return headers;
	}

	private formatMessages(messages: Message[]): Record<string, unknown>[] {
		const result: Record<string, unknown>[] = [];
		const serializedToolCallIds = new Set<string>();

		for (const msg of messages) {
			// Tool messages map 1:1 to API tool responses.
			// They contain only the execution output, so we bypass standard processing.
			if (msg.role === "tool") {
				for (const block of msg.blocks) {
					if (block.type === "tool_result" && serializedToolCallIds.has(block.toolCallId)) {
						result.push({
							role: "tool",
							tool_call_id: block.toolCallId,
							content: block.outputText,
						});
					}
				}
				continue;
			}

			const payload: Record<string, unknown> = { role: msg.role };
			const toolCalls: Record<string, unknown>[] = [];
			const contentArray: OpenAIContentPart[] = [];

			for (const block of msg.blocks) {
				switch (block.type) {
					case "tool_call":
						if (block.status === "complete") {
							toolCalls.push({
								id: block.toolCallId,
								type: "function",
								function: { name: block.name, arguments: block.argsText },
							});
							serializedToolCallIds.add(block.toolCallId);
						}
						break;

					case "text":
						contentArray.push({ type: "text", text: block.text });
						break;

					case "file":
						if (block.mimeType.startsWith("image/")) {
							contentArray.push({ type: "image_url", image_url: { url: block.data } });
						} else {
							contentArray.push({
								type: "text",
								text: `\n\n--- File: ${block.name || "Unknown"} ---\n${block.data}`,
							});
						}
						break;

					case "reasoning":
					case "artifact":
						// Intentionally omitted.
						// Reasoning tokens and internal UI artifacts are not sent back in context.
						break;
				}
			}

			if (msg.role === "assistant" && contentArray.length === 0 && toolCalls.length === 0) {
				continue;
			}

			if (toolCalls.length > 0) {
				payload.tool_calls = toolCalls;
			}
			// Conform to OpenAI's expected content structures
			if (msg.role === "assistant") {
				// Assistant messages strictly require a string or null (never an array)
				if (contentArray.length === 0) {
					payload.content = toolCalls.length > 0 ? null : "";
				} else {
					// Safely flatten any multiple text blocks into a single string
					payload.content = contentArray
						.filter((c) => c.type === "text")
						.map((c) => (c as { text: string }).text)
						.join("\n\n");
				}
			} else {
				// User messages can safely use the multimodal array format
				if (contentArray.length === 0) {
					payload.content = toolCalls.length > 0 ? null : "";
				} else if (contentArray.length === 1 && contentArray[0].type === "text") {
					// Fast path for simple text messages
					payload.content = contentArray[0].text;
				} else {
					// Multimodal or multi-part message
					payload.content = contentArray;
				}
			}

			result.push(payload);
		}
		return result;
	}

	private formatMessagesWithInstructions(messages: Message[], instructions?: string): Record<string, unknown>[] {
		const formattedMessages = this.formatMessages(messages);
		if (!instructions) return formattedMessages;
		return [{ role: "system", content: instructions }, ...formattedMessages];
	}

	private extractReasoning(delta: OpenAIStreamDelta): { text: string; encrypted: boolean } | null {
		// Check for encrypted reasoning (e.g., Anthropic via OpenRouter / Some DeepSeek setups)
		if (delta.reasoning && typeof delta.reasoning === "object" && typeof delta.reasoning.encrypted === "string") {
			return { text: "", encrypted: true };
		}
		if (typeof delta.reasoning_encrypted === "string") {
			return { text: "", encrypted: true };
		}

		// Check for standard reasoning
		for (const field of REASONING_FIELDS) {
			if (typeof delta[field] === "string" && delta[field].length > 0) {
				return { text: delta[field], encrypted: false };
			}
		}

		return null;
	}
}
