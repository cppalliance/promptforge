// The REC badge: present in the status bar, idle at boot, lit while the
// mic records, and cleared when the voice socket drops.
// Run: node test/rec-badge.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("the REC badge follows the recording", async ({ recEl, startTake, sleep, failures }) => {
  if (!recEl) {
    failures.push("status bar REC badge missing");
    return;
  }
  if (recEl.classList.contains("status-bar__rec--active")) {
    failures.push("the REC badge must start idle");
  }
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
    failures.push("starting voice capture did not light the REC badge");
  }
  voiceSocket.onclose?.();
  if (recEl.classList.contains("status-bar__rec--active")) {
    failures.push("a dropped voice socket did not clear the REC badge");
  }
});
