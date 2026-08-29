// Chat gating on the send path: while the last workbench snapshot says
// chat_ready: false, the composer blocks submission through the voice
// plugin's isSubmitBlocked hook - the send button disables and no chat
// frame leaves the socket - and a snapshot with chat_ready: true unblocks
// it again. The reason lives in the server's status frames; this test
// only guards the gate itself.
// Run: node test/chat-gating-submit.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("chat_ready gates the composer's send path", async (ctx) => {
  const { window, input, form, send, wsSocket, emitWorkbench, submitChat, sleep, failures } = ctx;

  input.value = "hold this";
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
  if (send.disabled) {
    failures.push("the send button is disabled while chat_ready is true");
  }

  emitWorkbench({ chat_ready: false });
  if (!send.disabled) {
    failures.push("the send button did not disable when chat_ready flipped false");
  }
  form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }));
  await sleep(50);
  if (wsSocket()?.chatFrame) {
    failures.push("a chat frame went out while chat_ready was false");
  }

  emitWorkbench({ chat_ready: true });
  if (send.disabled) {
    failures.push("the send button did not re-enable when chat_ready returned true");
  }
  const frame = await submitChat("hold this");
  if (!frame) {
    failures.push("submission stayed blocked after chat_ready returned true");
  }
});
