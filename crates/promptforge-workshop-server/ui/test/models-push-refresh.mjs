// Models frames: a pushed catalog (sent when the gateway comes back after
// an outage) refreshes the catalog without a fetch - and without touching
// the selection, which the server owns. The selection moves only when a
// workbench snapshot says so: a catalog push that drops the selected model
// changes nothing locally until the server's snapshot lands with the
// reconciled selection. Both sides are asserted observably through the
// Model menu: after a push its rows must render the pushed models, and
// the checked row must follow the workbench snapshots alone.
// Run: node test/models-push-refresh.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("models push refreshes the catalog, snapshots move the selection", async ({ document, emitModels, emitWorkbench, failures }) => {
  emitModels([
    { id: "fresh-model", description: "pushed" },
    { id: "test-model", description: "scripted" },
  ]);
  // The push must observably reach the Model menu - the onModels ->
  // setModels wiring in main.ts: open the menu and read its rows off the
  // catalog, plus which row carries the checked mark.
  const modelButton = document.querySelector('.window-titlebar__menu[data-menu="model"]');
  const menuState = () => {
    modelButton.click();
    const rows = [...modelButton.nextElementSibling.querySelectorAll('[role="menuitemradio"]')];
    const labels = rows.map(
      (row) => row.querySelector(".window-titlebar__item-label")?.textContent ?? "",
    );
    const checked = rows
      .filter((row) => row.getAttribute("aria-checked") === "true")
      .map((row) => row.querySelector(".window-titlebar__item-label")?.textContent ?? "");
    modelButton.click();
    return { labels, checked };
  };
  let state = menuState();
  if (!state.labels.includes("fresh-model") || !state.labels.includes("test-model")) {
    failures.push(`the pushed catalog did not render as Model menu rows: ${state.labels.join(",")}`);
  }
  if (state.checked.join(",") !== "test-model") {
    failures.push(`a catalog push moved the server-owned selection: ${state.checked.join(",")}`);
  }
  emitModels([{ id: "fresh-model", description: "pushed" }]);
  state = menuState();
  if (!state.labels.includes("fresh-model") || state.labels.includes("test-model")) {
    failures.push(`a narrowing catalog push did not replace the Model menu rows: ${state.labels.join(",")}`);
  }
  if (state.checked.length !== 0) {
    failures.push(`a catalog push that dropped the selection changed it locally: ${state.checked.join(",")}`);
  }
  emitWorkbench({ selected: "fresh-model" });
  state = menuState();
  if (state.checked.join(",") !== "fresh-model") {
    failures.push(`the workbench snapshot's selection did not take effect: ${state.checked.join(",")}`);
  }
});
