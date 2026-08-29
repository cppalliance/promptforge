// Chat gating on the mic: a workbench snapshot with chat_ready: false
// disables the mic button, and one arriving during a live take discards
// it - the REC badge clears, the voice socket closes, and the composer's
// readOnly lock lifts, because a take that cannot be sent is a trap.
// chat_ready: true re-enables the mic. Closing the Agent tab disposes the
// plugin's workbench subscription: a snapshot pushed after the close no
// longer drives the closed tab's mic.
// Run: node test/chat-gating-mic.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("chat_ready gates the mic and the gate dies with the tab", async (ctx) => {
  const { document, mic, input, recEl, FakeWebSocket, emitWorkbench, startTake, sleep, failures } = ctx;

  if (mic.disabled) {
    failures.push("the mic starts disabled while chat_ready is true");
  }

  emitWorkbench({ chat_ready: false });
  if (!mic.disabled) {
    failures.push("the mic did not disable when chat_ready flipped false");
  }
  emitWorkbench({ chat_ready: true });
  if (mic.disabled) {
    failures.push("the mic did not re-enable when chat_ready returned true");
  }

  const voiceSocket = await startTake();
  if (!voiceSocket) {
    failures.push("the mic click did not open a /voice socket");
    return;
  }
  voiceSocket.onmessage({ data: JSON.stringify({ type: "interim", committed: "hello", tentative: "" }) });
  if (!recEl.classList.contains("status-bar__rec--active")) {
    failures.push("REC badge not lit before the gate closed");
  }

  emitWorkbench({ chat_ready: false });
  if (!mic.disabled) {
    failures.push("the mic did not disable during the live take");
  }
  if (recEl.classList.contains("status-bar__rec--active")) {
    failures.push("REC badge not cleared when chat_ready gated the live take");
  }
  if (voiceSocket.readyState !== FakeWebSocket.CLOSED) {
    failures.push("the live take's voice socket was not closed by the gate");
  }
  if (input.readOnly) {
    failures.push("readOnly not lifted after the gated take was discarded");
  }

  emitWorkbench({ chat_ready: true });
  if (mic.disabled) {
    failures.push("the mic did not recover after the gate reopened");
  }

  // Close the Agent tab from its tab chip: the boot layout's only
  // closable default tab (the Workshop tree's permanent tab renders no
  // close action). Dockview closes the panel on the action's click.
  const closeAction = document.querySelector(".dv-default-tab-action");
  if (!closeAction) {
    failures.push("no closable tab action found for the Agent tab");
    return;
  }
  closeAction.click();
  // ChatUI.destroy is async; wait for the composer to unmount before
  // probing what a post-close snapshot still reaches.
  const closeDeadline = Date.now() + 2000;
  while (document.contains(mic) && Date.now() < closeDeadline) {
    await sleep(20);
  }
  if (document.contains(mic)) {
    failures.push("closing the Agent tab did not unmount its composer");
    return;
  }

  // ChatUI.destroy finishes (and disposes the subscription) a few ticks
  // after the composer unmounts, so probe until the deadline: emit a
  // gating snapshot and see whether it still drives the detached mic,
  // resetting between probes. A disposed subscription leaves the mic
  // untouched; a leaked one keeps flipping it until the deadline.
  const disposeDeadline = Date.now() + 2000;
  let leaked = true;
  while (Date.now() < disposeDeadline) {
    emitWorkbench({ chat_ready: false });
    if (!mic.disabled) {
      leaked = false;
      break;
    }
    emitWorkbench({ chat_ready: true });
    await sleep(20);
  }
  if (leaked) {
    failures.push("a snapshot after the tab closed still drove its mic: the subscription leaked");
  }
});
