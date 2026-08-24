import type { ChatProvider, ChatRequest, Message, StreamEvent } from "./chat/core/types";
import { uuidv7 } from "./chat/utils/uuid";

interface ServerFrame {
  type?: unknown;
  content?: unknown;
  message?: unknown;
}

/**
 * ChatProvider against the workbench's `GET /ws` WebSocket: one socket per
 * generation, opened on submit, carrying one `{"type":"chat",...}` frame and
 * answered by `delta`/`done`/`error` frames. Deliberately has no
 * `generateTitle`: titles cost an extra completion per chat and nothing in
 * the workbench UI displays them.
 */
export class WorkbenchProvider implements ChatProvider {
  async streamChat(request: ChatRequest, onEvent: (event: StreamEvent) => void): Promise<void> {
    const socket = new WebSocket(
      `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`,
    );
    // The engine's stop button aborts the signal; closing the socket is what
    // makes the server drop the gateway stream.
    const onAbort = (): void => socket.close();
    request.signal.addEventListener("abort", onAbort);
    try {
      await new Promise<void>((resolve, reject) => {
        const messageId = uuidv7();
        const textBlockId = uuidv7();
        let started = false;
        let settled = false;
        const settle = (fn: () => void): void => {
          if (settled) return;
          settled = true;
          fn();
        };

        socket.onopen = () => {
          socket.send(
            JSON.stringify({
              type: "chat",
              model: request.options.model,
              messages: formatMessages(request.messages),
            }),
          );
        };
        socket.onmessage = (event) => {
          let frame: ServerFrame;
          try {
            frame = JSON.parse(String(event.data)) as ServerFrame;
          } catch {
            // A non-JSON frame carries no chat event; keep reading.
            return;
          }
          if (frame.type === "delta" && typeof frame.content === "string" && frame.content !== "") {
            if (!started) {
              started = true;
              onEvent({
                type: "message_start",
                message: { id: messageId, role: "assistant", blocks: [] },
              });
            }
            onEvent({ type: "text_delta", messageId, blockId: textBlockId, delta: frame.content });
            return;
          }
          if (frame.type === "done") {
            settle(() => {
              if (started) onEvent({ type: "finish", reason: "stop" });
              resolve();
            });
            return;
          }
          if (frame.type === "error") {
            settle(() =>
              reject(
                new Error(
                  typeof frame.message === "string" && frame.message !== ""
                    ? frame.message
                    : "the chat stream failed",
                ),
              ),
            );
          }
        };
        socket.onerror = () => {
          settle(() => reject(new Error("the chat socket failed")));
        };
        socket.onclose = () => {
          settle(() => {
            // The engine records an abort itself; the provider just stops.
            if (request.signal.aborted) {
              resolve();
              return;
            }
            // A close without a terminal frame mirrors an SSE body that ends
            // early: finish what was streamed, or fail when nothing arrived.
            if (started) {
              onEvent({ type: "finish", reason: "stop" });
              resolve();
              return;
            }
            reject(new Error("the chat socket closed before the reply completed"));
          });
        };
      });
    } finally {
      request.signal.removeEventListener("abort", onAbort);
      socket.close();
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
