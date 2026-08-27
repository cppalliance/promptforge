// readOnly during takes: the composer locks against typing while a take is
// live (input.readOnly true), the interim still lands programmatically,
// and stopping the take via the mic button clears readOnly once the final
// arrives, leaving the final text in place.
// Run: node test/voice-readonly-take.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("the composer is readOnly during a take", async ({ input, mic, startTake, failures }) => {
  input.value = "prefix";
  input.setSelectionRange(6, 6);
  const takeSocket = await startTake();
  if (!takeSocket) {
    failures.push("the mic click did not open a /voice socket");
    return;
  }
  if (!input.readOnly) {
    failures.push("input.readOnly must be true during the take");
  }
  takeSocket.onmessage({ data: JSON.stringify({ type: "interim", committed: " world", tentative: "" }) });
  if (input.value !== "prefix world") {
    failures.push(`expected "prefix world", got "${input.value}"`);
  }
  // Stop via mic click (triggers stopVoice), then the final arrives.
  mic.click();
  takeSocket.onmessage({ data: JSON.stringify({ type: "final", text: " world" }) });
  if (input.readOnly) {
    failures.push("readOnly not cleared after the final");
  }
  if (input.value !== "prefix world") {
    failures.push(`final text wrong, got "${input.value}"`);
  }
});
