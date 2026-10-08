// Unit test for the shared focus-trapped modal (modal.ts): the overlay and
// dialog structure with the prefix class contract, the Tab trap cycling
// both directions, Escape and backdrop dismissal with focus return to the
// invoker, the requiresValue gating with Enter submission, the per-kind
// duplicate guard, the `primary` flag, and the two skins (the confirmation
// prompt puts the primary button first, the Windows order; the form modal
// puts Cancel first and the primary last). Bundles the module with esbuild
// and drives it against jsdom.
// Run: node test/shared-modal.mjs (from crates/workshop/look).
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const lookDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.HTMLElement = window.HTMLElement;
globalThis.HTMLButtonElement = window.HTMLButtonElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  entryPoints: [path.join(lookDir, "modal.ts")],
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

const container = window.document.createElement("div");
window.document.body.append(container);
const invoker = window.document.createElement("button");
invoker.textContent = "open";
window.document.body.append(invoker);
invoker.focus();

// --- Structure and the prefix class contract -------------------------------------

let chosen = null;
let dismissed = 0;
const handle = openModal({
  container,
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

const overlay = container.querySelector(".confirm-overlay");
check("the overlay mounts into the container", overlay !== null);
check("the overlay has the shared base class", overlay?.classList.contains("modal-overlay"));
const dialog = container.querySelector(".confirm");
check("the dialog has the shared base class", dialog?.classList.contains("modal-dialog"));
check("the dialog is an alertdialog", dialog?.getAttribute("role") === "alertdialog");
check("the dialog is modal", dialog?.getAttribute("aria-modal") === "true");
check("the dialog labels by the title", dialog?.getAttribute("aria-labelledby") === "confirm-title");
check(
  "the dialog describes by the message",
  dialog?.getAttribute("aria-describedby") === container.querySelector(".confirm__line")?.id,
);
check("the title renders", container.querySelector(".confirm__title")?.textContent === "Delete the model?");
check("the message renders", container.querySelector(".confirm__line")?.textContent === "This removes the model.");
check("the actions have the prefix class", container.querySelector(".confirm__actions") !== null);
check("focus lands on the first button", window.document.activeElement?.textContent === "Cancel");

// --- The duplicate guard ----------------------------------------------------------

const second = openModal({
  container,
  classPrefix: "confirm",
  titleId: "confirm-title",
  title: "Again?",
  message: "no",
  buttons: [{ label: "OK", run: () => undefined }],
});
check("a second dialog of the same kind is a no-op", container.querySelectorAll(".confirm-overlay").length === 1);
check("the duplicate handle reads closed", second.closed === true);

// --- The Tab trap cycles both directions -------------------------------------------
// jsdom has no default Tab navigation, so only the trap's boundary wraps
// are observable: Tab on the last button, Shift+Tab on the first.

const cancelButton = [...container.querySelectorAll(".confirm__actions button")].find(
  (button) => button.textContent === "Cancel",
);
const deleteButton = [...container.querySelectorAll(".confirm__actions button")].find(
  (button) => button.textContent === "Delete",
);
deleteButton.focus();
pressKey(window.document, "Tab");
check("Tab on the last button wraps to the first", window.document.activeElement === cancelButton);
pressKey(window.document, "Tab", true);
check("Shift+Tab on the first button wraps to the last", window.document.activeElement === deleteButton);

// --- Escape dismisses with focus return ----------------------------------------------

pressKey(window.document, "Escape");
check("Escape dismisses the dialog", container.querySelector(".confirm-overlay") === null);
check("Escape fires onDismiss", dismissed === 1);
check("Escape runs no button", chosen === null);
check("Escape returns focus to the invoker", window.document.activeElement === invoker);
check("the handle reads closed after dismissal", handle.closed === true);

// --- The backdrop dismisses; the card does not ---------------------------------------

openModal({
  container,
  classPrefix: "confirm",
  titleId: "confirm-title",
  title: "t",
  message: "m",
  dismissOnBackdrop: true,
  onDismiss: () => (dismissed += 1),
  buttons: [{ label: "OK", className: "button", run: () => (chosen = true) }],
});
container.querySelector(".confirm").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
check("a click on the card keeps the dialog open", container.querySelector(".confirm-overlay") !== null);
container.querySelector(".confirm-overlay").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
check("a backdrop click dismisses", container.querySelector(".confirm-overlay") === null);
check("the backdrop fires onDismiss", dismissed === 2);

// --- The field: requiresValue gating and Enter submission -----------------------------

let fieldValue = null;
openModal({
  container,
  classPrefix: "ws-workspace-add",
  titleId: "workspace-add-title",
  title: "Add Folder",
  message: "Enter the path.",
  field: { id: "workspace-add-path", label: "Folder path" },
  buttons: [
    { label: "Add", requiresValue: true, run: (value) => (fieldValue = value) },
    { label: "Cancel", run: () => undefined },
  ],
});
const input = container.querySelector("#workspace-add-path");
const addButton = [...container.querySelectorAll(".ws-workspace-add__button")].find(
  (button) => button.textContent === "Add",
);
check("the field renders with its label", container.querySelector(".ws-workspace-add__label") !== null);
check("focus lands on the field", window.document.activeElement === input);
check("the gated button starts disabled", addButton?.disabled === true);
pressKey(input, "Enter");
check("Enter with an empty field submits nothing", fieldValue === null);
input.value = "  C:\\models  ";
input.dispatchEvent(new window.Event("input", { bubbles: true }));
check("typing enables the gated button", addButton?.disabled === false);
pressKey(input, "Enter");
check("Enter submits the trimmed value", fieldValue === "C:\\models");
check("the submission dismissed the dialog", container.querySelector(".ws-workspace-add-overlay") === null);

// --- The primary flag ------------------------------------------------------------------

const labelsOf = (prefix) =>
  [...container.querySelectorAll(`.${prefix}__actions button`)].map((button) => button.textContent);

openModal({
  container,
  classPrefix: "plain",
  titleId: "plain-title",
  title: "t",
  message: "m",
  buttons: [
    { label: "Cancel", run: () => undefined },
    { label: "Go", primary: true, run: () => undefined },
  ],
});
const plainGo = [...container.querySelectorAll(".plain__actions button")].find(
  (button) => button.textContent === "Go",
);
check("a primary button takes the --primary modifier", plainGo?.classList.contains("plain__button--primary"));
check(
  "a non-primary button has no --primary modifier",
  !container.querySelector(".plain__actions button")?.classList.contains("plain__button--primary"),
);
check("focus lands on the primary button when there is no field", window.document.activeElement === plainGo);
check("an unskinned dialog keeps the given order", labelsOf("plain").join(",") === "Cancel,Go");
check("an unskinned dialog has no skin classes", container.querySelector(".plain-overlay")?.className === "modal-overlay plain-overlay");
pressKey(window.document, "Escape");

// --- The confirmation skin: primary first, the Windows order --------------------------------

function openConfirmation(buttons) {
  return openModal({
    container,
    classPrefix: "confirm-skin",
    titleId: "confirm-skin-title",
    title: "Do you want to save the changes you made to a.md?",
    message: "Your changes will be lost if you don't save them.",
    skin: "confirmation",
    buttons,
  });
}

openConfirmation([
  { label: "Save", primary: true, run: () => undefined },
  { label: "Don't Save", run: () => undefined },
  { label: "Cancel", run: () => undefined },
]);
check(
  "the confirmation overlay carries the skin class",
  container.querySelector(".confirm-skin-overlay")?.classList.contains("modal-overlay--confirmation"),
);
check(
  "the confirmation dialog carries the skin class",
  container.querySelector(".confirm-skin")?.classList.contains("modal-dialog--confirmation"),
);
check(
  "the Windows order is Save, Don't Save, Cancel",
  labelsOf("confirm-skin").join("|") === "Save|Don't Save|Cancel",
);
check(
  "focus lands on the primary Save",
  window.document.activeElement?.textContent === "Save",
);
const skinButtons = [...container.querySelectorAll(".confirm-skin__actions button")];
check(
  "the primary button takes the primary control classes",
  skinButtons[0].classList.contains("button") && skinButtons[0].classList.contains("button-primary"),
);
check(
  "a plain button takes the secondary control classes",
  skinButtons[1].classList.contains("button") && skinButtons[1].classList.contains("button-secondary"),
);
pressKey(window.document, "Escape");

openConfirmation([
  { label: "Cancel", run: () => undefined },
  { label: "Don't Save", run: () => undefined },
  { label: "Save", primary: true, run: () => undefined },
]);
check(
  "the confirmation skin moves the primary button first and keeps the rest in order",
  labelsOf("confirm-skin").join("|") === "Save|Cancel|Don't Save",
);
pressKey(window.document, "Escape");

openConfirmation([
  { label: "Revert", danger: true, run: () => undefined },
  { label: "Cancel", run: () => undefined },
]);
const revertButtons = [...container.querySelectorAll(".confirm-skin__actions button")];
check(
  "a danger button takes the danger control class and the --danger modifier",
  revertButtons[0].classList.contains("button-danger") &&
    revertButtons[0].classList.contains("confirm-skin__button--danger"),
);
check(
  "a skinned dialog with no primary button keeps the given order",
  labelsOf("confirm-skin").join("|") === "Revert|Cancel",
);
pressKey(window.document, "Escape");

// --- The form skin: Cancel first, the primary last --------------------------------------------

let formValue = null;
openModal({
  container,
  classPrefix: "form-skin",
  titleId: "form-skin-title",
  title: "Add Folder",
  message: "Enter the path.",
  skin: "form",
  field: { id: "form-skin-path", label: "Folder path" },
  buttons: [
    { label: "Add", primary: true, requiresValue: true, run: (value) => (formValue = value) },
    { label: "Cancel", run: () => undefined },
  ],
});
check(
  "the form overlay carries the skin class",
  container.querySelector(".form-skin-overlay")?.classList.contains("modal-overlay--form"),
);
check(
  "the form dialog carries the skin class",
  container.querySelector(".form-skin")?.classList.contains("modal-dialog--form"),
);
check("the form skin puts Cancel first and the primary last", labelsOf("form-skin").join("|") === "Cancel|Add");
check("the gated primary starts disabled", container.querySelector(".form-skin__actions button:last-child")?.disabled === true);
const formInput = container.querySelector("#form-skin-path");
check("focus lands on the field", window.document.activeElement === formInput);
check("a skinned field's input takes the input control class", formInput.classList.contains("input"));
formInput.value = "C:\\work";
formInput.dispatchEvent(new window.Event("input", { bubbles: true }));
pressKey(formInput, "Enter");
check("Enter submits through the primary button wherever it sits", formValue === "C:\\work");

// --- Enter in a field dialog whose primary button has no requiresValue -------------------------
// The form skin reorders the buttons to Cancel, Save, so the first button is Cancel. With no
// value-gated button, Enter has to take the primary button and not fall through to the first one.

const plainRuns = [];
openModal({
  container,
  classPrefix: "form-plain",
  titleId: "form-plain-title",
  title: "Rename",
  message: "Enter the new name.",
  skin: "form",
  field: { id: "form-plain-name", label: "Name" },
  buttons: [
    { label: "Save", primary: true, run: (value) => plainRuns.push(`Save:${value}`) },
    { label: "Cancel", run: () => plainRuns.push("Cancel") },
  ],
});
check(
  "a form dialog with a plain primary puts Cancel first, so the first button is not the primary",
  labelsOf("form-plain").join("|") === "Cancel|Save",
);
const plainInput = container.querySelector("#form-plain-name");
plainInput.value = "notes";
pressKey(plainInput, "Enter");
check("Enter runs the plain primary button with the field value", plainRuns.join(",") === "Save:notes");
check("Enter does not run Cancel, the first button after reordering", !plainRuns.includes("Cancel"));
check("Enter dismisses the dialog", container.querySelector(".form-plain-overlay") === null);

// An unskinned dialog keeps the given order, so the primary is not first there either.
const unskinnedRuns = [];
openModal({
  container,
  classPrefix: "field-plain",
  titleId: "field-plain-title",
  title: "Rename",
  message: "Enter the new name.",
  field: { id: "field-plain-name", label: "Name" },
  buttons: [
    { label: "Cancel", run: () => unskinnedRuns.push("Cancel") },
    { label: "Save", primary: true, run: (value) => unskinnedRuns.push(`Save:${value}`) },
  ],
});
pressKey(container.querySelector("#field-plain-name"), "Enter");
check(
  "Enter takes the primary button in an unskinned dialog too, not the first",
  unskinnedRuns.join(",") === "Save:",
);

if (failures.length > 0) {
  console.error(`shared-modal: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("shared-modal: all assertions passed");
