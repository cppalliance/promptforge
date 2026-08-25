import type { ChatState, ContentBlock, Message, StreamEvent } from "./types";

type StreamErrorEvent = {
	type: "error";
	message: string;
};

export type StreamReducerEvent = StreamEvent | StreamErrorEvent;

function clearEphemeralFlag(msg: Message): void {
	if (!msg.ephemeral) return;
	delete msg.ephemeral;
}

function touchMessage(msg: Message, timestamp = Date.now()): void {
	msg.createdAt ??= timestamp;
	msg.updatedAt = timestamp;
}

function updateStreamingToolCalls(msg: Message, status: Extract<ContentBlock, { type: "tool_call" }>["status"]): void {
	for (const block of msg.blocks) {
		if (block.type === "tool_call" && block.status === "streaming") {
			block.status = status;
		}
	}
}

function findMessage(state: ChatState, messageId: string | null | undefined): Message | undefined {
	if (!messageId) return undefined;

	const lastMessage = state.messages[state.messages.length - 1];
	if (lastMessage?.id === messageId) return lastMessage;

	return state.messages.find((m) => m.id === messageId);
}

function adoptMessageId(state: ChatState, msg: Message, nextId: string): void {
	const previousId = msg.id;
	msg.id = nextId;
	if (state.generatingMessageId === previousId) {
		state.generatingMessageId = nextId;
	}
}

function canAdoptMessageId(state: ChatState, msg: Message, nextId: string): boolean {
	if (!msg.ephemeral) return false;
	if (msg.blocks.length > 0) return false;
	return findMessage(state, nextId) === undefined;
}

function pushStreamMessage(
	state: ChatState,
	message: Pick<Message, "id" | "role" | "blocks" | "meta" | "runId" | "createdAt" | "updatedAt">,
	fallbackRunId?: string,
): Message {
	const timestamp = Date.now();
	const createdAt = message.createdAt ?? timestamp;
	const msg: Message = {
		id: message.id,
		role: message.role,
		blocks: [],
		runId: message.runId ?? fallbackRunId,
		createdAt,
		updatedAt: message.updatedAt ?? createdAt,
		...(message.role === "assistant" && message.blocks.length === 0 ? { ephemeral: true } : {}),
	};
	state.messages.push(msg);
	if (msg.role === "assistant") {
		state.generatingMessageId = msg.id;
	}
	return msg;
}

function eventMessageId(event: StreamReducerEvent): string | null {
	switch (event.type) {
		case "message_start":
			return event.message.id;
		case "usage":
		case "finish":
		case "error":
			return null;
		default:
			return event.messageId;
	}
}

export function applyStreamEventToState(state: ChatState, currentMessageId: string, event: StreamReducerEvent): string {
	let msg = findMessage(state, currentMessageId) ?? findMessage(state, state.generatingMessageId);
	if (!msg) return currentMessageId;

	// Let the empty local placeholder take the provider/adaptor message id,
	// or switch to a new stream message when a later event starts one.
	const nextMessageId = eventMessageId(event);
	if (nextMessageId && msg.id !== nextMessageId) {
		if (canAdoptMessageId(state, msg, nextMessageId)) {
			adoptMessageId(state, msg, nextMessageId);
		} else if (!findMessage(state, nextMessageId)) {
			updateStreamingToolCalls(msg, "complete");
			touchMessage(msg);
			msg =
				event.type === "message_start"
					? pushStreamMessage(state, event.message, msg.runId)
					: pushStreamMessage(state, { id: nextMessageId, role: "assistant", blocks: [] }, msg.runId);
		} else if (event.type === "message_start") {
			return msg.id;
		}
	}

	switch (event.type) {
		case "message_start": {
			msg.runId = event.message.runId ?? msg.runId;
			msg.createdAt ??= event.message.createdAt ?? Date.now();
			if (event.message.updatedAt !== undefined) {
				msg.updatedAt = event.message.updatedAt;
			}
			msg.role = event.message.role;
			if (event.message.blocks.length > 0 || msg.blocks.length === 0) {
				msg.blocks = event.message.blocks;
			}
			if (event.message.meta) {
				msg.meta = { ...msg.meta, ...event.message.meta };
			}
			if (event.message.blocks.length > 0) {
				clearEphemeralFlag(msg);
			} else if (msg.role === "assistant" && msg.blocks.length === 0) {
				msg.ephemeral = true;
			}
			if (msg.role === "assistant") {
				state.generatingMessageId = msg.id;
			}
			touchMessage(msg, event.message.updatedAt ?? Date.now());
			break;
		}

		case "text_delta": {
			let tb = msg.blocks.find((b) => b.id === event.blockId) as Extract<ContentBlock, { type: "text" }>;
			if (!tb) {
				tb = { id: event.blockId, type: "text", text: "" };
				msg.blocks.push(tb);
			}
			tb.text += event.delta;
			if (event.delta.length > 0) {
				clearEphemeralFlag(msg);
				touchMessage(msg);
			}
			break;
		}

		case "reasoning_delta": {
			let rb = msg.blocks.find((b) => b.id === event.blockId) as Extract<ContentBlock, { type: "reasoning" }>;
			if (!rb) {
				rb = { id: event.blockId, type: "reasoning", text: "", encrypted: event.encrypted };
				msg.blocks.push(rb);
			}
			if (event.encrypted) {
				rb.encrypted = true;
				if (event.delta) {
					rb.encryptedText = (rb.encryptedText ?? "") + event.delta;
				}
			} else {
				rb.text += event.delta;
			}
			if (event.delta.length > 0) {
				clearEphemeralFlag(msg);
				touchMessage(msg);
			}
			break;
		}

		case "tool_call_start":
			msg.blocks.push(event.block);
			clearEphemeralFlag(msg);
			touchMessage(msg);
			break;

		case "tool_call_delta": {
			const tcb = msg.blocks.find((b) => b.id === event.blockId) as Extract<ContentBlock, { type: "tool_call" }>;
			if (tcb) {
				if (event.name !== undefined) tcb.name = event.name;
				if (event.argsDelta) tcb.argsText += event.argsDelta;
				if (event.status) tcb.status = event.status;
				if (event.name !== undefined || event.argsDelta || event.status) {
					clearEphemeralFlag(msg);
					touchMessage(msg);
				}
			}
			break;
		}

		case "tool_result":
		case "artifact":
			msg.blocks.push(event.block);
			clearEphemeralFlag(msg);
			touchMessage(msg);
			break;
		case "usage":
			msg.usage = {
				input: event.input,
				output: event.output,
				total: event.total ?? event.input + event.output,
				...(event.cacheRead !== undefined ? { cacheRead: event.cacheRead } : {}),
				...(event.cacheWrite !== undefined ? { cacheWrite: event.cacheWrite } : {}),
				...(event.details !== undefined ? { details: event.details } : {}),
			};
			touchMessage(msg);
			break;
		case "finish": {
			const finalStatus = event.reason === "error" || event.reason === "aborted" ? "error" : "complete";
			updateStreamingToolCalls(msg, finalStatus);
			touchMessage(msg);
			break;
		}
		case "error":
			state.error = { message: event.message, id: msg.id };
			updateStreamingToolCalls(msg, "error");
			touchMessage(msg);
			break;
	}

	return msg.id;
}
