// Models frames: a pushed catalog (sent when the gateway comes back after
// an outage) refreshes the model service without a fetch - and without
// touching the selection, which the server owns. The selection moves only
// when a workbench snapshot says so: a catalog push that drops the
// selected model changes nothing locally until the server's snapshot
// lands with the reconciled selection. Both sides are asserted observably
// through the agent toolbar's model picker: after a push its dropdown
// lists the pushed models, and the trigger label follows the workbench
// snapshots alone.
// Run: node test/models-push-refresh.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("models push refreshes the picker's catalog, snapshots move the selection", async ({ document, emitModels, emitWorkbench, failures }) => {
  const trigger = document.querySelector("#dock .ws-agent-panel .ws-model-picker-trigger");
  if (!trigger) {
    failures.push("the agent toolbar's model picker trigger did not mount");
    return;
  }
  const label = () => trigger.querySelector(".ws-model-picker-trigger__label")?.textContent ?? "";
  // Opens the picker's dropdown, reads the row labels, toggles it closed.
  const pickerRows = () => {
    trigger.click();
    const rows = [...document.querySelectorAll(".menu-popup .menu-item__label")].map(
      (el) => el.textContent ?? "",
    );
    trigger.click();
    return rows;
  };

  if (label() !== "test-model") {
    failures.push(`the boot snapshot's selection did not reach the picker: "${label()}"`);
  }

  // The push must observably reach the picker - the socket's onModels feed
  // into the model service: open the dropdown and read its rows off the
  // catalog, while the label keeps the server-owned selection.
  emitModels([
    { id: "fresh-model", description: "pushed" },
    { id: "test-model", description: "scripted" },
  ]);
  let rows = pickerRows();
  if (!rows.includes("fresh-model") || !rows.includes("test-model")) {
    failures.push(`the pushed catalog did not render as picker rows: ${rows.join(",")}`);
  }
  if (label() !== "test-model") {
    failures.push(`a catalog push moved the server-owned selection: "${label()}"`);
  }

  emitModels([{ id: "fresh-model", description: "pushed" }]);
  rows = pickerRows();
  if (!rows.includes("fresh-model") || rows.includes("test-model")) {
    failures.push(`a narrowing catalog push did not replace the picker rows: ${rows.join(",")}`);
  }
  if (label() !== "test-model") {
    failures.push(`a catalog push that dropped the selection changed it locally: "${label()}"`);
  }

  emitWorkbench({ selected: "fresh-model" });
  if (label() !== "fresh-model") {
    failures.push(`the workbench snapshot's selection did not take effect: "${label()}"`);
  }
});
