// Models frames: a pushed catalog (sent when the gateway comes back after
// an outage) refreshes the catalog without a fetch - and without touching
// the selection, which the server owns. The selection moves only when a
// workbench snapshot says so: a catalog push that drops the selected model
// changes nothing locally until the server's snapshot lands with the
// reconciled selection.
// Run: node test/models-push-refresh.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("models push refreshes the catalog, snapshots move the selection", async ({ emitModels, emitWorkbench, submitChat, failures }) => {
  emitModels([
    { id: "fresh-model", description: "pushed" },
    { id: "test-model", description: "scripted" },
  ]);
  let request = await submitChat("still there?");
  if (request?.model !== "test-model") {
    failures.push(`a catalog push moved the server-owned selection: ${request?.model}`);
  }
  emitModels([{ id: "fresh-model", description: "pushed" }]);
  request = await submitChat("once more?");
  if (request?.model !== "test-model") {
    failures.push(`a catalog push that dropped the selection changed it locally: ${request?.model}`);
  }
  emitWorkbench({ selected: "fresh-model" });
  request = await submitChat("after the snapshot?");
  if (request?.model !== "fresh-model") {
    failures.push(`the workbench snapshot's selection did not take effect: ${request?.model}`);
  }
});
