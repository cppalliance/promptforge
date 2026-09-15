// Workbench snapshots drive the Model menu's idleness through the
// composition root's profileMenu view: a frame with switch_in_flight true
// and switching null (a switch to no profile) must disable every model
// and profile row and mark "No profile" pending, and the next idle frame
// must restore them. Asserted through the real title-bar menu after a
// boot, so the main.ts adapter that forwards switchInFlight from the
// snapshot is on the path - a constant there would fail this test while
// the view-only window-menu tests still pass.
// Run: node test/profile-switch-in-flight.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("a no-profile switch in flight disables the Model menu through the booted adapter", async ({ document, emitWorkbench, failures }) => {
  const modelButton = document.querySelector('.ws-window-titlebar__menu[data-menu="model"]');
  // Opens the menu, reads the radio rows, closes it again.
  const menuState = () => {
    modelButton.click();
    const rows = [...modelButton.nextElementSibling.querySelectorAll('[role="menuitemradio"]')];
    const state = rows.map((row) => ({
      label: row.querySelector(".ws-window-titlebar__item-label")?.textContent ?? "",
      disabled: row.getAttribute("aria-disabled") === "true",
      busy: row.getAttribute("aria-busy") === "true",
      mark: row.querySelector(".ws-window-titlebar__item-check")?.textContent ?? "",
    }));
    modelButton.click();
    return state;
  };

  emitWorkbench({
    profiles: ["main", "coding"],
    active: "main",
    switching: null,
    switch_in_flight: true,
    chat_ready: false,
  });
  let rows = menuState();
  const labels = rows.map((row) => row.label);
  if (!labels.includes("No profile") || !labels.includes("main") || !labels.includes("coding")) {
    failures.push(`the Model menu did not list the pushed profiles: ${labels.join(",")}`);
    return;
  }
  if (!rows.every((row) => row.disabled)) {
    failures.push(
      `a no-profile switch in flight left rows enabled: ${rows.filter((row) => !row.disabled).map((row) => row.label).join(",")}`,
    );
  }
  const noProfile = rows.find((row) => row.label === "No profile");
  if (!noProfile.busy || noProfile.mark !== "…") {
    failures.push(
      `the No profile row is not pending during a no-profile switch: busy=${noProfile.busy} mark="${noProfile.mark}"`,
    );
  }
  const otherBusy = rows.filter((row) => row.busy && row.label !== "No profile");
  if (otherBusy.length > 0) {
    failures.push(`rows other than No profile are pending: ${otherBusy.map((row) => row.label).join(",")}`);
  }

  emitWorkbench({
    profiles: ["main", "coding"],
    active: null,
    switching: null,
    switch_in_flight: false,
    chat_ready: true,
  });
  rows = menuState();
  if (!rows.every((row) => !row.disabled)) {
    failures.push(
      `an idle snapshot left rows disabled: ${rows.filter((row) => row.disabled).map((row) => row.label).join(",")}`,
    );
  }
  if (rows.some((row) => row.busy || row.mark === "…")) {
    failures.push(`an idle snapshot left a row pending: ${rows.filter((row) => row.busy).map((row) => row.label).join(",")}`);
  }
});
