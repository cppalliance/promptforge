// The recording LED and activity LED stand in one indicators group beside
// the barberpole, never behind it: a busy frame shows the barberpole
// and leaves the group and both LEDs visible, the barberpole precedes the
// group in DOM order, and a non-busy frame hides the barberpole alone.
// Run: node test/barberpole-beside-indicators.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("the barberpole shows beside the recording and activity LEDs", async (ctx) => {
  const { emitStatus, barberpoleEl, indicatorsEl, recEl, ledEl, failures } = ctx;
  if (!indicatorsEl) {
    failures.push("status bar indicators group missing");
    return;
  }
  if (!barberpoleEl) {
    failures.push("status bar barberpole missing");
    return;
  }
  if (indicatorsEl.hidden) {
    failures.push("the indicators group must start visible");
  }
  if (!barberpoleEl.hidden) {
    failures.push("the barberpole must start hidden");
  }
  const following = barberpoleEl.compareDocumentPosition(indicatorsEl);
  if ((following & barberpoleEl.DOCUMENT_POSITION_FOLLOWING) === 0) {
    failures.push("the barberpole does not precede the indicators group in DOM order");
  }

  emitStatus({
    label: "Downloading model",
    description: "1 of 2",
    activity: "general",
    busy: true,
  });
  if (barberpoleEl.hidden) {
    failures.push("a busy frame did not reveal the barberpole");
  }
  if (indicatorsEl.hidden) {
    failures.push("a busy frame hid the recording and activity LED group");
  }
  if (recEl.hidden || ledEl.hidden) {
    failures.push("a busy frame hid an LED individually");
  }

  emitStatus({ label: "Download complete", description: "ready" });
  if (!barberpoleEl.hidden) {
    failures.push("a non-busy frame did not hide the barberpole");
  }
  if (indicatorsEl.hidden) {
    failures.push("a non-busy frame hid the recording and activity LED group");
  }
});
