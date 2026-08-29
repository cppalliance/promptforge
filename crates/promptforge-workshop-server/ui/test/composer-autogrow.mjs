// Composer auto-grow: an interim transcript rewrites the textarea
// programmatically, and the box must grow to fit it. The voice path
// notifies murm-ui's Input through a dispatched "input" event; in jsdom
// (no CSS global in Node) murm-ui takes its adjustHeight path, which the
// boot helper's scrollHeight shim turns into an observable inline height.
// Run: node test/composer-autogrow.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("the composer grows on a multiline interim", async ({ input, startTake, failures }) => {
  const takeSocket = await startTake();
  if (!takeSocket) {
    failures.push("the mic click did not open a /voice socket with a message listener");
    return;
  }
  const interimText = "line one\nline two\nline three";
  const heightBefore = parseFloat(input.style.height) || 0;
  takeSocket.onmessage({
    data: JSON.stringify({ type: "interim", committed: interimText, tentative: "" }),
  });
  const heightAfter = parseFloat(input.style.height) || 0;
  if (input.value !== interimText) {
    failures.push("the interim transcript did not land in the composer");
  }
  if (!(heightAfter > heightBefore)) {
    failures.push(
      `the composer did not grow on a multiline interim (was ${input.style.height || "unset"})`,
    );
  }
  takeSocket.onclose?.();
});
