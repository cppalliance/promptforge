// The REC badge and activity LED live in one indicators group that swaps
// out as a unit behind the progress bar: a progress frame hides the group
// (not its members individually), and clearing progress restores it with a
// live recording's REC state intact.
// Run: node test/progress-swap-indicators.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("the REC badge and LED swap out as one group behind the progress bar", async (ctx) => {
  const { emitStatus, progressEl, indicatorsEl, recEl, ledEl, startTake, sleep, failures } = ctx;
  if (!indicatorsEl) {
    failures.push("status bar indicators group missing");
    return;
  }
  if (indicatorsEl.hidden) {
    failures.push("the indicators group must start visible");
  }

  // Light the REC badge with a live recording before the swap.
  const voiceSocket = await startTake();
  if (!voiceSocket) {
    failures.push("no /voice socket was opened");
    return;
  }
  const recDeadline = Date.now() + 5000;
  while (!recEl.classList.contains("status-bar__rec--active") && Date.now() < recDeadline) {
    await sleep(20);
  }
  if (!recEl.classList.contains("status-bar__rec--active")) {
    failures.push("voice capture did not light the REC badge before the swap");
  }

  emitStatus({
    label: "Downloading model",
    description: "1 of 2",
    activity: "general",
    progress: { current: 1, total: 2 },
  });
  if (!indicatorsEl.hidden) {
    failures.push("a progress frame did not hide the REC and LED group");
  }
  if (progressEl.hidden) {
    failures.push("a progress frame did not reveal the progress bar");
  }
  if (recEl.hidden || ledEl.hidden) {
    failures.push("the swap hid the badge or LED individually instead of the group");
  }
  if (!recEl.classList.contains("status-bar__rec--active")) {
    failures.push("the swap disturbed the REC badge's recording state");
  }

  emitStatus({ label: "Download complete", description: "ready" });
  if (indicatorsEl.hidden) {
    failures.push("clearing progress did not restore the REC and LED group");
  }
  if (!progressEl.hidden) {
    failures.push("clearing progress did not hide the progress bar");
  }
  if (!recEl.classList.contains("status-bar__rec--active")) {
    failures.push("the REC badge lost its recording state across the swap");
  }

  voiceSocket.onclose?.();
});
