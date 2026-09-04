// Unit test for the shared focus-trapped modal (shared-ui/modal.ts):
// the overlay and dialog structure with the prefix class contract, the
// Tab trap cycling both directions, Escape and backdrop dismissal with
// focus return to the invoker, the requiresValue gating with Enter
// submission, and the per-kind duplicate guard. Bundles the module with
// esbuild and drives it against jsdom.
// Run: node test/shared-modal.mjs.
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.HTMLElement = window.HTMLElement;
globalThis.HTMLButtonElement = window.HTMLButtonElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "node_modules", "shared-ui", "modal.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The module imports its colocated CSS; the test drives only the JS,
  // and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});
const { openModal } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function pressKey(target, key, shiftKey = false) {
  target.dispatchEvent(
    new window.KeyboardEvent("keydown", { key, shiftKey, bubbles: true, cancelable: true }),
  );
}

const host = window.document.createElement("div");
window.document.body.append(host);
const invoker = window.document.createElement("button");
invoker.textContent = "open";
window.document.body.append(invoker);
invoker.focus();

// --- Structure and the prefix class contract -------------------------------------

let chosen = null;
let dismissed = 0;
const handle = openModal({
  host,
  classPrefix: "confirm",
  titleId: "confirm-title",
  title: "Delete the model?",
  message: "This removes the model.",
  role: "alertdialog",
  dismissOnBackdrop: true,
  onDismiss: () => (dismissed += 1),
  buttons: [
    { label: "Cancel", className: "button button-outline", run: () => (chosen = false) },
    { label: "Delete", className: "button button-danger", run: () => (chosen = true) },
  ],
});

const overlay = host.querySelector(".confirm-overlay");
check("the overlay mounts into the host", overlay !== null);
check("the overlay carries the shared base class", overlay?.classList.contains("modal-overlay"));
const dialog = host.querySelector(".confirm");
check("the dialog carries the shared base class", dialog?.classList.contains("modal-dialog"));
check("the dialog is an alertdialog", dialog?.getAttribute("role") === "alertdialog");
check("the dialog is modal", dialog?.getAttribute("aria-modal") === "true");
check("the dialog labels by the title", dialog?.getAttribute("aria-labelledby") === "confirm-title");
check(
  "the dialog describes by the message",
  dialog?.getAttribute("aria-describedby") === host.querySelector(".confirm__line")?.id,
);
check("the title renders", host.querySelector(".confirm__title")?.textContent === "Delete the model?");
check("the message renders", host.querySelector(".confirm__line")?.textContent === "This removes the model.");
check("the actions carry the prefix class", host.querySelector(".confirm__actions") !== null);
check("focus lands on the first button", window.document.activeElement?.textContent === "Cancel");

// --- The duplicate guard ----------------------------------------------------------

const second = openModal({
  host,
  classPrefix: "confirm",
  titleId: "confirm-title",
  title: "Again?",
  message: "no",
  buttons: [{ label: "OK", run: () => undefined }],
});
check("a second dialog of the same kind is a no-op", host.querySelectorAll(".confirm-overlay").length === 1);
check("the duplicate handle reads closed", second.closed === true);

// --- The Tab trap cycles both directions -------------------------------------------
// jsdom has no default Tab navigation, so only the trap's boundary wraps
// are observable: Tab on the last button, Shift+Tab on the first.

const cancelButton = [...host.querySelectorAll(".confirm__actions button")].find(
  (button) => button.textContent === "Cancel",
);
const deleteButton = [...host.querySelectorAll(".confirm__actions button")].find(
  (button) => button.textContent === "Delete",
);
deleteButton.focus();
pressKey(window.document, "Tab");
check("Tab on the last button wraps to the first", window.document.activeElement === cancelButton);
pressKey(window.document, "Tab", true);
check("Shift+Tab on the first button wraps to the last", window.document.activeElement === deleteButton);

// --- Escape dismisses with focus return ----------------------------------------------

pressKey(window.document, "Escape");
check("Escape dismisses the dialog", host.querySelector(".confirm-overlay") === null);
check("Escape fires onDismiss", dismissed === 1);
check("Escape runs no button", chosen === null);
check("Escape returns focus to the invoker", window.document.activeElement === invoker);
check("the handle reads closed after dismissal", handle.closed === true);

// --- The backdrop dismisses; the card does not ---------------------------------------

openModal({
  host,
  classPrefix: "confirm",
  titleId: "confirm-title",
  title: "t",
  message: "m",
  dismissOnBackdrop: true,
  onDismiss: () => (dismissed += 1),
  buttons: [{ label: "OK", className: "button", run: () => (chosen = true) }],
});
host.querySelector(".confirm").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
check("a click on the card keeps the dialog open", host.querySelector(".confirm-overlay") !== null);
host.querySelector(".confirm-overlay").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
check("a backdrop click dismisses", host.querySelector(".confirm-overlay") === null);
check("the backdrop fires onDismiss", dismissed === 2);

// --- The field: requiresValue gating and Enter submission -----------------------------

let fieldValue = null;
openModal({
  host,
  classPrefix: "workspace-add",
  titleId: "workspace-add-title",
  title: "Add Folder",
  message: "Enter the path.",
  field: { id: "workspace-add-path", label: "Folder path" },
  buttons: [
    { label: "Add", requiresValue: true, run: (value) => (fieldValue = value) },
    { label: "Cancel", run: () => undefined },
  ],
});
const input = host.querySelector("#workspace-add-path");
const addButton = [...host.querySelectorAll(".workspace-add__button")].find(
  (button) => button.textContent === "Add",
);
check("the field renders with its label", host.querySelector(".workspace-add__label") !== null);
check("focus lands on the field", window.document.activeElement === input);
check("the gated button starts disabled", addButton?.disabled === true);
pressKey(input, "Enter");
check("Enter with an empty field submits nothing", fieldValue === null);
input.value = "  C:\\models  ";
input.dispatchEvent(new window.Event("input", { bubbles: true }));
check("typing enables the gated button", addButton?.disabled === false);
pressKey(input, "Enter");
check("Enter submits the trimmed value", fieldValue === "C:\\models");
check("the submission dismissed the dialog", host.querySelector(".workspace-add-overlay") === null);

if (failures.length > 0) {
  console.error(`shared-modal: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("shared-modal: all assertions passed");
