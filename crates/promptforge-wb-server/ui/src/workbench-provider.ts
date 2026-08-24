import type { ChatProvider, ChatRequest, Message, StreamEvent } from "./chat/core/types";
import { parseSSE } from "./chat/utils/sse";
import { uuidv7 } from "./chat/utils/uuid";

interface SseChunk {
  id?: string;
  choices?: Array<{ delta?: { content?: unknown } }>;
}

/**
 * ChatProvider against the workbench's `POST /chat`: the request body is the
 * OpenAI-shaped `{model, messages, stream: true}` and the reply is the
 * gateway's SSE stream relayed event-for-event. Deliberately has no
 * `generateTitle`: titles cost an extra completion per chat and nothing in
 * the workbench UI displays them.
 */
export class WorkbenchProvider implements ChatProvider {
  async streamChat(request: ChatRequest, onEvent: (event: StreamEvent) => void): Promise<void> {
    const response = await fetch("/chat", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        model: request.options.model,
        messages: formatMessages(request.messages),
        stream: true,
      }),
      signal: request.signal,
    });
    if (!response.ok) {
      const detail = await response.text();
      throw new Error(`POST /chat answered ${response.status}: ${detail}`);
    }

    const messageId = uuidv7();
    const textBlockId = uuidv7();
    let started = false;

    await parseSSE(response, (data) => {
      if (data === "[DONE]") return undefined;
      let chunk: SseChunk;
      try {
        chunk = JSON.parse(data) as SseChunk;
      } catch {
        // A non-JSON data payload carries no chat delta; keep reading.
        return undefined;
      }
      const delta = chunk.choices?.[0]?.delta?.content;
      if (typeof delta !== "string" || delta === "") return undefined;
      if (!started) {
        started = true;
        onEvent({
          type: "message_start",
          message: { id: messageId, role: "assistant", blocks: [] },
        });
      }
      onEvent({ type: "text_delta", messageId, blockId: textBlockId, delta });
      return undefined;
    });

    if (started) {
      onEvent({ type: "finish", reason: "stop" });
    }
  }
}

// Flattens each message's text blocks into the OpenAI `{role, content}`
// shape; messages with no text (the ephemeral streaming placeholder) are
// dropped.
function formatMessages(messages: readonly Message[]): Array<{ role: string; content: string }> {
  const formatted: Array<{ role: string; content: string }> = [];
  for (const message of messages) {
    const text = message.blocks
      .filter((block) => block.type === "text")
      .map((block) => (block as { text: string }).text)
      .join("\n\n");
    if (text === "") continue;
    formatted.push({ role: message.role, content: text });
  }
  return formatted;
}
