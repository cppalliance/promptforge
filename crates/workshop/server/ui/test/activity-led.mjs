// The activity LED: generating pulses light it green, thinking pulses
// amber, and green wins while both are lit inside one pulse window. Debug
// frames pulse it too. After the window the LED returns to its idle lens.
// The pulse window defaults to 250ms here (jsdom loads no stylesheet), so
// 400ms of silence guarantees decay.
// Run: node test/activity-led.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("the activity LED pulses and decays", async ({ emitStatus, ledEl, sleep, failures }) => {
  const ledLit = () =>
    ledEl.classList.contains("status-bar__led--generating") ||
    ledEl.classList.contains("status-bar__led--thinking");

  // Let any pulse from boot-time frames decay before asserting idle.
  await sleep(400);
  if (ledLit()) failures.push("the LED did not start idle after the pulse window");
  emitStatus({ label: "delta", severity: "debug", activity: "generating" });
  if (!ledEl.classList.contains("status-bar__led--generating")) {
    failures.push("generating activity did not light the LED green");
  }
  emitStatus({ label: "thinking", severity: "debug", activity: "thinking" });
  if (!ledEl.classList.contains("status-bar__led--generating")) {
    failures.push("green did not win while generating and thinking were both lit");
  }
  if (ledEl.classList.contains("status-bar__led--thinking")) {
    failures.push("the thinking modifier applied while generating was lit");
  }
  await sleep(400);
  if (ledLit()) failures.push("the LED stayed lit past the pulse window");
  emitStatus({ label: "thinking", severity: "debug", activity: "thinking" });
  if (!ledEl.classList.contains("status-bar__led--thinking")) {
    failures.push("thinking activity did not light the LED amber");
  }
  await sleep(400);
  if (ledLit()) failures.push("the LED stayed lit past the pulse window");
});
