import type { JsonValue, Message } from "./types";

/** Extracts all plain text from text blocks */
export function extractPlainText(msg: Message): string {
	return msg.blocks
		.filter((b) => b.type === "text")
		.map((b) => b.text)
		.join("\n\n");
}

export function dropEphemeralMessages(messages: Message[]): Message[] {
	return messages.filter((m) => !m.ephemeral);
}

export function cloneMessages(messages: Message[]): Message[] {
	return messages.map((message) => {
		const cloned: Message = {
			...message,
			blocks: message.blocks.map((block) => ({ ...block })),
		};
		if (message.usage) {
			cloned.usage = {
				...message.usage,
				...(message.usage.details !== undefined ? { details: cloneJsonValue(message.usage.details) } : {}),
			};
		}
		if (message.meta) {
			cloned.meta = cloneJsonValue(message.meta);
		}
		return cloned;
	});
}

function cloneJsonValue<T extends JsonValue>(value: T): T {
	if (Array.isArray(value)) {
		return value.map((item) => cloneJsonValue(item)) as T;
	}
	if (value && typeof value === "object") {
		const cloned: { [key: string]: JsonValue } = {};
		for (const [key, item] of Object.entries(value)) {
			cloned[key] = cloneJsonValue(item);
		}
		return cloned as T;
	}
	return value;
}
