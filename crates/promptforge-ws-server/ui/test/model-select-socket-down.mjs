// A model-row click while the workshop socket is down must not fail
// silently: ModelService.setCurrent returns false when nothing went out,
// and the composition root's Model-menu view surfaces that on the status
// bar - the same seam a failed profile switch uses. Drives the real
// title-bar menu against a downed socket and reads the status-bar text.
// Run: node test/model-select-socket-down.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("a model selection while the socket is down surfaces on the status bar", async ({ document, wsSocket, FakeWebSocket, statusText, failures }) => {
  // Down the socket from the send path's perspective without firing
  // onclose: sendFrame checks readyState, and a fired close would start
  // the reconnect and status-reset machinery this test does not exercise.
  wsSocket().readyState = FakeWebSocket.CLOSED;
  const modelButton = document.querySelector('.window-titlebar__menu[data-menu="model"]');
  modelButton.click();
  const row = [...modelButton.nextElementSibling.querySelectorAll(".window-titlebar__item")]
    .find((item) => item.querySelector(".window-titlebar__item-label").textContent === "test-model");
  if (!row) {
    failures.push("the Model menu never listed test-model");
    return;
  }
  row.click();
  if (statusText.textContent !== "Could not select test-model: the workshop socket is down") {
    failures.push(`the status bar did not surface the failed selection: "${statusText.textContent}"`);
  }
  if (!statusText.classList.contains("status-bar__text--error")) {
    failures.push("the surfaced selection failure is not styled as an error");
  }
});
