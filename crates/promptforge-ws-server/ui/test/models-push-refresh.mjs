// Models frames: a pushed catalog (sent when the gateway comes back after
// an outage) refreshes the catalog without a fetch - and without touching
// the selection, which the server owns. The selection moves only when a
// workbench snapshot says so: a catalog push that drops the selected model
// changes nothing locally until the server's snapshot lands with the
// reconciled selection. The catalog side is asserted observably: after a
// push, the Model menu's rows must render the pushed models.
// Run: node test/models-push-refresh.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("models push refreshes the catalog, snapshots move the selection", async ({ document, emitModels, emitWorkbench, submitChat, failures }) => {
  emitModels([
    { id: "fresh-model", description: "pushed" },
    { id: "test-model", description: "scripted" },
  ]);
  // The push must observably reach the Model menu - the onModels ->
  // setModels wiring in main.ts, not just the selection state the other
  // assertions read: open the menu and read its rows off the catalog.
  const modelButton = document.querySelector('.window-titlebar__menu[data-menu="model"]');
  const menuRows = () => {
    modelButton.click();
    const rows = [...modelButton.nextElementSibling.querySelectorAll(".window-titlebar__item-label")]
      .map((label) => label.textContent);
    modelButton.click();
    return rows;
  };
  let rows = menuRows();
  if (rows.join(",") !== "fresh-model,test-model") {
    failures.push(`the pushed catalog did not render as Model menu rows: ${rows.join(",")}`);
  }
  let request = await submitChat("still there?");
  if (request?.model !== "test-model") {
    failures.push(`a catalog push moved the server-owned selection: ${request?.model}`);
  }
  emitModels([{ id: "fresh-model", description: "pushed" }]);
  rows = menuRows();
  if (rows.join(",") !== "fresh-model") {
    failures.push(`a narrowing catalog push did not replace the Model menu rows: ${rows.join(",")}`);
  }
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
