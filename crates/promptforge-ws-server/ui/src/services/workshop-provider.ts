import type { ChatProvider, ChatRequest, Message, StreamEvent } from "../chat/core/types";
import { uuidv7 } from "../chat/utils/uuid";
import type { WorkshopSocket } from "./workshop-socket";

/**
 * ChatProvider against the workshop's persistent `/ws` socket: each
 * generation is one id-tagged chat frame on the shared connection, answered
 * by `delta`/`reasoning`/`done`/`error` frames carrying that id, while
 * status frames bypass the chat entirely. Reasoning frames become
 * `reasoning_delta` events in their own block, which the Thinking plugin
 * renders. Deliberately has no `generateTitle`: titles cost an extra
 * completion per chat and nothing in the workshop UI displays them.
 */
export class WorkshopProvider implements ChatProvider {
  constructor(private readonly socket: WorkshopSocket) {}

  async streamChat(request: ChatRequest, onEvent: (event: StreamEvent) => void): Promise<void> {
    const messageId = uuidv7();
    const textBlockId = uuidv7();
    const reasoningBlockId = uuidv7();
    let started = false;
    const ensureStarted = (): void => {
      if (started) return;
      started = true;
      onEvent({
        type: "message_start",
        message: { id: messageId, role: "assistant", blocks: [] },
      });
    };
    await this.socket.streamChat(
      {
        // Submit is blocked in the UI without a model; the empty string is
        // the unreachable default for the type.
        model: request.options.model ?? "",
        messages: formatMessages(request.messages),
      },
      {
        onDelta: (content) => {
          ensureStarted();
          onEvent({ type: "text_delta", messageId, blockId: textBlockId, delta: content });
        },
        onReasoning: (content) => {
          ensureStarted();
          onEvent({
            type: "reasoning_delta",
            messageId,
            blockId: reasoningBlockId,
            delta: content,
          });
        },
      },
      request.signal,
    );
    // An aborted generation is recorded by the engine itself; the provider
    // finishes only a reply that ran to its done frame.
    if (started && !request.signal.aborted) {
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
