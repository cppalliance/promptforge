// Committed/tentative interims: the textarea shows committed + tentative,
// joined with a space only when the committed prefix does not already end
// in whitespace. Committed is append-only within a take, so the display
// follows the server unconditionally - a shorter tentative never shrinks
// the text while committed keeps growing.
// Run: node test/voice-interim-splice.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("interim transcripts splice committed and tentative", async ({ input, startTake, failures }) => {
  const takeSocket = await startTake();
  if (!takeSocket) {
    failures.push("the mic click did not open a /voice socket with a message listener");
    return;
  }
  const sendInterim = (committed, tentative) =>
    takeSocket.onmessage({ data: JSON.stringify({ type: "interim", committed, tentative }) });
  sendInterim("One two.", "three");
  if (input.value !== "One two. three") {
    failures.push(`committed+tentative did not join with a space: "${input.value}"`);
  }
  sendInterim("One two. three four.", "");
  if (input.value !== "One two. three four.") {
    failures.push(`a grown committed prefix did not land verbatim: "${input.value}"`);
  }
  const grownLength = input.value.length;
  sendInterim("One two. three four. five six.", "se");
  if (input.value !== "One two. three four. five six. se") {
    failures.push(`a shorter tentative with grown committed mis-rendered: "${input.value}"`);
  }
  if (input.value.length <= grownLength) {
    failures.push("the text shrank while committed kept growing");
  }
  sendInterim("One two. three four. five six. ", "seven");
  if (input.value !== "One two. three four. five six. seven") {
    failures.push(`a trailing-whitespace committed prefix gained a double space: "${input.value}"`);
  }
  sendInterim("", "fresh start");
  if (input.value !== "fresh start") {
    failures.push(`an empty committed prefix gained a leading space: "${input.value}"`);
  }
  takeSocket.onclose?.();
});
