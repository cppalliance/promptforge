// The recording LED and activity LED live in one indicators group that
// swaps out as a unit behind the progress bar: a progress frame hides the
// group (not its members individually), and clearing progress restores it.
// Run: node test/progress-swap-indicators.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("the recording and activity LEDs swap out as one group behind the progress bar", async (ctx) => {
  const { emitStatus, progressEl, indicatorsEl, recEl, ledEl, failures } = ctx;
  if (!indicatorsEl) {
    failures.push("status bar indicators group missing");
    return;
  }
  if (indicatorsEl.hidden) {
    failures.push("the indicators group must start visible");
  }

  emitStatus({
    label: "Downloading model",
    description: "1 of 2",
    activity: "general",
    progress: { current: 1, total: 2 },
  });
  if (!indicatorsEl.hidden) {
    failures.push("a progress frame did not hide the recording and activity LED group");
  }
  if (progressEl.hidden) {
    failures.push("a progress frame did not reveal the progress bar");
  }
  if (recEl.hidden || ledEl.hidden) {
    failures.push("the swap hid an LED individually instead of the group");
  }

  emitStatus({ label: "Download complete", description: "ready" });
  if (indicatorsEl.hidden) {
    failures.push("clearing progress did not restore the recording and activity LED group");
  }
  if (!progressEl.hidden) {
    failures.push("clearing progress did not hide the progress bar");
  }
});
