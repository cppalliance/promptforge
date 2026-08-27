// Models frames: a pushed catalog (sent when the gateway comes back after
// an outage) refreshes the selection state without a fetch. A selection
// that survives the new catalog is kept; one that vanished falls back to
// the first entry. The pushed catalogs order the surviving entry second so
// preservation is distinguishable from a plain first-entry fallback.
// Run: node test/models-push-refresh.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("models push refreshes the selection", async ({ emitModels, submitChat, failures }) => {
  emitModels([
    { id: "fresh-model", description: "pushed" },
    { id: "test-model", description: "scripted" },
  ]);
  let request = await submitChat("still there?");
  if (request?.model !== "test-model") {
    failures.push(`a surviving selection was not kept across the refresh: ${request?.model}`);
  }
  emitModels([{ id: "fresh-model", description: "pushed" }]);
  request = await submitChat("once more?");
  if (request?.model !== "fresh-model") {
    failures.push(`a vanished selection did not fall back to the first entry: ${request?.model}`);
  }
});
