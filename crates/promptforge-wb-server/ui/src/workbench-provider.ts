import type { ChatProvider, ChatRequest, Message, StreamEvent } from "./chat/core/types";
import { uuidv7 } from "./chat/utils/uuid";
import type { WorkbenchSocket } from "./workbench-socket";

/**
 * ChatProvider against the workbench's persistent `/ws` socket: each
 * generation is one id-tagged chat frame on the shared connection, answered
 * by `delta`/`done`/`error` frames carrying that id, while status frames
 * bypass the chat entirely. Deliberately has no `generateTitle`: titles
 * cost an extra completion per chat and nothing in the workbench UI
 * displays them.
 */
export class WorkbenchProvider implements ChatProvider {
  constructor(private readonly socket: WorkbenchSocket) {}

  async streamChat(request: ChatRequest, onEvent: (event: StreamEvent) => void): Promise<void> {
    const messageId = uuidv7();
    const textBlockId = uuidv7();
    let started = false;
    await this.socket.streamChat(
      {
        // Submit is blocked in the UI without a model; the empty string is
        // the unreachable default for the type.
        model: request.options.model ?? "",
        messages: formatMessages(request.messages),
      },
      (content) => {
        if (!started) {
          started = true;
          onEvent({
            type: "message_start",
            message: { id: messageId, role: "assistant", blocks: [] },
          });
        }
        onEvent({ type: "text_delta", messageId, blockId: textBlockId, delta: content });
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
