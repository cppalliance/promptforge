// Chat gating on the mic: the mic stays visible and clickable whether or
// not chat is ready - a click with chat_ready: false starts no take and
// names the blocker on the status bar instead of the control disappearing.
// A snapshot arriving during a live take still discards it - the REC badge
// clears, the voice socket closes, and the composer's readOnly lock lifts,
// because a take that cannot be sent is a trap. Closing the Agent tab
// disposes the plugin: a click on the detached mic starts nothing.
// Run: node test/chat-gating-mic.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("chat_ready gates the mic's click and the wiring dies with the tab", async (ctx) => {
  const { document, mic, input, recEl, statusText, FakeWebSocket, emitWorkbench, startTake, sleep, failures } = ctx;

  if (mic.disabled) {
    failures.push("the mic starts enabled while chat_ready is true");
  }

  // A gated click starts no take and explains itself on the status bar.
  emitWorkbench({ chat_ready: false });
  const gated = await startTake();
  if (gated) {
    failures.push("a mic click with chat_ready false opened a /voice socket");
  }
  if (!statusText.textContent.includes("Chat isn't ready")) {
    failures.push(`a gated click named no blocker on the status bar (got "${statusText.textContent}")`);
  }

  emitWorkbench({ chat_ready: true });
  const voiceSocket = await startTake();
  if (!voiceSocket) {
    failures.push("the mic click did not open a /voice socket once chat_ready returned");
    return;
  }
  voiceSocket.onmessage({ data: JSON.stringify({ type: "interim", committed: "hello", tentative: "" }) });
  if (!recEl.classList.contains("status-bar__rec--active")) {
    failures.push("REC badge not lit before the gate closed");
  }

  emitWorkbench({ chat_ready: false });
  if (recEl.classList.contains("status-bar__rec--active")) {
    failures.push("REC badge not cleared when chat_ready gated the live take");
  }
  if (voiceSocket.readyState !== FakeWebSocket.CLOSED) {
    failures.push("the live take's voice socket was not closed by the gate");
  }
  if (input.readOnly) {
    failures.push("readOnly not lifted after the gated take was discarded");
  }

  // The mic is never disabled, so there is nothing to re-enable: with the
  // gate open again a click starts a fresh take.
  emitWorkbench({ chat_ready: true });
  const reopened = await startTake();
  if (!reopened) {
    failures.push("the mic click did not start a fresh take after the gate reopened");
    return;
  }
  reopened.close();

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
  // probing what a click on the detached mic still reaches.
  const closeDeadline = Date.now() + 2000;
  while (document.contains(mic) && Date.now() < closeDeadline) {
    await sleep(20);
  }
  if (document.contains(mic)) {
    failures.push("closing the Agent tab did not unmount its composer");
    return;
  }

  // The disposed voice handle answers nothing: clicking the detached mic
  // starts no take.
  const afterClose = await startTake();
  if (afterClose) {
    failures.push("a click on the closed tab's detached mic started a take");
  }
});
