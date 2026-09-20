// The live "upstream connect error" repro: a Thinking status frame sustains
// the amber LED past its pulse window (the decay re-adds the sustained
// state and clears the timer), then an error frame with general activity
// arrives with no pulse pending. The error frame must return the LED to
// idle instead of orphaning the amber glow indefinitely.
// The pulse window defaults to 250ms here (jsdom loads no stylesheet), so
// 400ms of silence guarantees the window has expired.
// Run: node test/led-error-after-thinking.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench(
  "an error frame after the pulse window returns the LED to idle",
  async ({ emitStatus, ledEl, sleep, failures }) => {
    const ledLit = () =>
      ledEl.classList.contains("status-bar__led--generating") ||
      ledEl.classList.contains("status-bar__led--thinking");

    // Let any pulse from boot-time frames decay before starting the repro.
    await sleep(400);
    if (ledLit()) failures.push("the LED did not start idle after the pulse window");
    emitStatus({ label: "Thinking", severity: "info", activity: "thinking" });
    if (!ledEl.classList.contains("status-bar__led--thinking")) {
      failures.push("the thinking status frame did not light the LED amber");
    }
    // The pulse window expires; the decay re-adds the sustained thinking
    // state and clears the timer, so the amber glow survives.
    await sleep(400);
    if (!ledEl.classList.contains("status-bar__led--thinking")) {
      failures.push("the sustained thinking state did not outlive the pulse window");
    }
    emitStatus({
      label: "upstream connect error",
      description: "the gateway rejected the completion",
      severity: "error",
      activity: "general",
    });
    if (ledLit()) failures.push("the error frame left the LED stuck amber");
    await sleep(400);
    if (ledLit()) failures.push("the LED re-lit after the error frame");
  },
);
