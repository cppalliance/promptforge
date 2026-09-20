// The recording LED and activity LED stand in one indicators group beside
// the barberpole, never behind it: a progress frame shows the barberpole
// and leaves the group and both LEDs visible, the barberpole precedes the
// group in DOM order, and clearing progress hides the barberpole alone.
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
    progress: { current: 1, total: 2 },
  });
  if (barberpoleEl.hidden) {
    failures.push("a progress frame did not reveal the barberpole");
  }
  if (indicatorsEl.hidden) {
    failures.push("a progress frame hid the recording and activity LED group");
  }
  if (recEl.hidden || ledEl.hidden) {
    failures.push("a progress frame hid an LED individually");
  }

  emitStatus({ label: "Download complete", description: "ready" });
  if (!barberpoleEl.hidden) {
    failures.push("clearing progress did not hide the barberpole");
  }
  if (indicatorsEl.hidden) {
    failures.push("clearing progress hid the recording and activity LED group");
  }
});
