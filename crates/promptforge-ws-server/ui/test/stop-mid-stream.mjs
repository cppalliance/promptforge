// Stop mid-stream: with a generation hanging (one delta, no done), the
// send button becomes a stop button and the observer's thinking frame
// holds the amber LED. Pressing Stop aborts the chat and recycles the
// socket, so no terminal status frame ever arrives for it; the bar must
// clear its own activity LED on the abort and stay idle once the stream's
// leftover pulse timers settle.
// Run: node test/stop-mid-stream.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("aborting a hung stream clears the LED", async (ctx) => {
  const { window, input, form, send, ledEl, emitStatus, wsSocket, FakeWebSocket, sleep, failures } = ctx;

  const liveSocket = wsSocket();
  const openDeadline = Date.now() + 5000;
  while (liveSocket.readyState !== FakeWebSocket.OPEN && Date.now() < openDeadline) {
    await sleep(20);
  }
  if (liveSocket.readyState !== FakeWebSocket.OPEN) {
    failures.push("the /ws socket never opened");
    return;
  }
  liveSocket.hangChat = true;
  input.value = "hang please";
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
  form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }));
  // Wait for the chat frame on the wire, not the stop button's label:
  // the label flips synchronously on submit, but the engine's request
  // preparation is async, so the socket stream exists a few ticks later.
  const sendDeadline = Date.now() + 5000;
  while (!liveSocket.chatFrame && Date.now() < sendDeadline) {
    await sleep(20);
  }
  if (!liveSocket.chatFrame) {
    failures.push("the hanging chat frame was never sent");
    return;
  }
  if (send.getAttribute("aria-label") !== "Stop generation") {
    failures.push("the send button never became a stop button");
    return;
  }
  emitStatus({
    label: "Streaming response...",
    description: "the gateway is streaming the reply",
    activity: "thinking",
  });
  if (!ledEl.classList.contains("status-bar__led--thinking")) {
    failures.push("the thinking frame did not light the LED amber");
  }
  // The stop button is type=submit with no click handler of its own;
  // dispatching the form's submit routes to the same handler, which
  // sees the in-flight generation and stops it.
  form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }));
  if (
    ledEl.classList.contains("status-bar__led--generating") ||
    ledEl.classList.contains("status-bar__led--thinking")
  ) {
    failures.push("the LED stayed lit through the abort");
  }
  // Let any pulse timer left over from the stream settle; the LED
  // must stay idle rather than be re-armed by a stale sustained state.
  await sleep(400);
  if (
    ledEl.classList.contains("status-bar__led--generating") ||
    ledEl.classList.contains("status-bar__led--thinking")
  ) {
    failures.push("the LED re-lit after the abort once timers settled");
  }
});
