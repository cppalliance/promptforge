// Status frames render into the status bar. Text and tooltip: info and
// error frames set the bar text and description tooltip, error frames style
// the text and the styling clears on the next info frame, and debug frames
// are internal instrumentation that must not touch either. Busy: a
// non-null progress shows the barberpole beside the recording+activity LED
// indicators group, which stays visible; a null progress hides the
// barberpole; debug frames never disturb it.
// Run: node test/status-frames.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("status frames render into the bar", async (ctx) => {
  const { emitStatus, statusText, statusBar, barberpoleEl, indicatorsEl, failures } = ctx;

  emitStatus({
    label: "Streaming response...",
    description: "gateway stream open",
    activity: "thinking",
  });
  if (statusText.textContent !== "Streaming response...") {
    failures.push("a status frame did not update the bar text");
  }
  if (statusBar.title !== "gateway stream open") {
    failures.push("the status description did not land on the bar tooltip");
  }
  emitStatus({ label: "per-delta pulse", description: "debug", severity: "debug", activity: "generating" });
  if (statusText.textContent !== "Streaming response...") {
    failures.push("a debug status frame changed the bar text");
  }
  if (statusBar.title !== "gateway stream open") {
    failures.push("a debug status frame changed the tooltip");
  }
  emitStatus({
    label: "Gateway error: 500",
    description: "upstream declined",
    severity: "error",
    activity: "general",
  });
  if (statusText.textContent !== "Gateway error: 500") {
    failures.push("an error frame did not update the bar text");
  }
  if (!statusText.classList.contains("status-bar__text--error")) {
    failures.push("an error frame did not style the bar text");
  }
  emitStatus({ label: "Ready", description: "idle" });
  if (statusText.classList.contains("status-bar__text--error")) {
    failures.push("the error styling did not clear on the next info frame");
  }

  emitStatus({
    label: "Downloading model",
    description: "1 of 4",
    activity: "general",
    progress: { current: 1, total: 4 },
  });
  if (barberpoleEl.hidden) failures.push("a progress frame did not reveal the barberpole");
  if (indicatorsEl.hidden) {
    failures.push("the recording and activity LED group hid while the barberpole is showing");
  }
  emitStatus({
    label: "Downloading model",
    description: "2 of 4",
    activity: "general",
    progress: { current: 2, total: 4 },
  });
  emitStatus({ label: "per-delta pulse", severity: "debug", activity: "generating" });
  if (barberpoleEl.hidden) {
    failures.push("a debug frame disturbed the barberpole");
  }
  emitStatus({ label: "Download complete", description: "ready" });
  if (!barberpoleEl.hidden) failures.push("a null-progress frame did not hide the barberpole");
  if (indicatorsEl.hidden) {
    failures.push("the recording and activity LED group hid when progress cleared");
  }
});
