// Discard on send: submitting a chat while recording discards the live
// take - the REC badge clears, the voice socket closes, the composer's
// readOnly lock lifts, and a final frame arriving late does not write into
// the textarea.
// Run: node test/voice-discard-on-send.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("submitting a chat discards the live take", async (ctx) => {
  const { window, input, form, recEl, FakeWebSocket, startTake, sleep, failures } = ctx;

  input.value = "";
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
  const discardSocket = await startTake();
  if (!discardSocket) {
    failures.push("the mic click did not open a /voice socket");
    return;
  }
  discardSocket.onmessage({ data: JSON.stringify({ type: "interim", committed: "hello", tentative: "" }) });
  if (!recEl.classList.contains("status-bar__rec--active")) {
    failures.push("REC badge not lit before submit");
  }
  input.value = "send this";
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
  form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }));
  const submitDeadline = Date.now() + 2000;
  while (recEl.classList.contains("status-bar__rec--active") && Date.now() < submitDeadline) {
    await sleep(20);
  }
  if (recEl.classList.contains("status-bar__rec--active")) {
    failures.push("REC badge not cleared after submit");
  }
  if (discardSocket.readyState !== FakeWebSocket.CLOSED) {
    failures.push("the voice socket was not closed");
  }
  if (input.readOnly) {
    failures.push("readOnly not cleared after the discard");
  }
  const valueBeforeLate = input.value;
  discardSocket.onmessage?.({ data: JSON.stringify({ type: "final", text: "LATE FINAL" }) });
  if (input.value !== valueBeforeLate) {
    failures.push("a late final frame wrote into the textarea after the discard");
  }
});
