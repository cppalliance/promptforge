// Unit test for the shared toast stack (toast.ts): show() puts a kind-classed
// toast on top of a polite live region, the stack keeps the newest three,
// each toast leaves after its severity's lifetime (info and success 15s,
// warning 18s, error 20s), hovering or focusing a toast pauses its timer
// and the remaining time resumes on leave, a close X dismisses at once, and
// every kind carries its codicon glyph. Bundles the module with esbuild and
// drives it against jsdom with mocked timers and a mocked clock.
// Run: node test/shared-toast.mjs (from crates/workshop/look).
import path from "node:path";
import { mock } from "node:test";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const lookDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.HTMLElement = window.HTMLElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { createToastStack } from "./toast.ts";
      export { ICON_CHECK, ICON_ERROR, ICON_INFO, ICON_WARNING } from "./icons.ts";
    `,
    resolveDir: lookDir,
    loader: "ts",
  },
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
const { createToastStack, ICON_CHECK, ICON_ERROR, ICON_INFO, ICON_WARNING } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// The hover pause measures elapsed time with the clock, so it is mocked too.
mock.timers.enable({ apis: ["setTimeout", "Date"] });

/** Builds a stack in the document, answering it with its toasts, newest first. */
function mount() {
  window.document.body.replaceChildren();
  const stack = createToastStack();
  window.document.body.append(stack.element);
  return { stack, toasts: () => [...stack.element.querySelectorAll(".toast")] };
}

const hover = (toast, type) => toast.dispatchEvent(new window.MouseEvent(type, { bubbles: false }));

// --- The stack and one toast ---------------------------------------------------

{
  const { stack, toasts } = mount();
  check("the stack is a polite live region", stack.element.getAttribute("aria-live") === "polite");
  check("the stack announces as status", stack.element.getAttribute("role") === "status");

  stack.show("PromptForge 1.2.3 is available", "info");
  const [toast] = toasts();
  check("show appends one toast per call", toasts().length === 1);
  check("the toast has its kind class", toast?.classList.contains("toast-info") === true);
  check(
    "the toast renders its message in its own element",
    toast?.querySelector(".toast__message")?.textContent === "PromptForge 1.2.3 is available",
  );
  check("the toast's text is only its message", toast?.textContent === "PromptForge 1.2.3 is available");
  check("the toast has a close button", toast?.querySelector("button.toast__close")?.type === "button");
  check(
    "the close button is named for assistive tech",
    toast?.querySelector("button.toast__close")?.getAttribute("aria-label") === "Clear Notification",
  );
  check("the close button carries an X glyph", toast?.querySelector(".toast__close svg") !== null);
  check("the glyph is hidden from assistive tech", toast?.querySelector(".toast__icon")?.getAttribute("aria-hidden") === "true");
}

// --- Every kind has its codicon -------------------------------------------------

for (const [kind, icon] of [
  ["info", ICON_INFO],
  ["warning", ICON_WARNING],
  ["error", ICON_ERROR],
  ["success", ICON_CHECK],
]) {
  const { stack, toasts } = mount();
  stack.show("glyph", kind);
  const [toast] = toasts();
  // The DOM re-serializes the icon string, so compare it with the same string parsed the same way.
  const expected = window.document.createElement("span");
  expected.innerHTML = icon;
  check(`a ${kind} toast has its kind class`, toast?.classList.contains(`toast-${kind}`) === true);
  check(
    `a ${kind} toast draws its codicon`,
    expected.querySelector("path") !== null && toast?.querySelector(".toast__icon")?.innerHTML === expected.innerHTML,
  );
}

// --- Newest on top, three visible -----------------------------------------------

{
  const { stack, toasts } = mount();
  const text = () => toasts().map((toast) => toast.querySelector(".toast__message").textContent);
  stack.show("first", "info");
  stack.show("second", "info");
  check("the newest toast is on top", text().join(",") === "second,first");
  stack.show("third", "info");
  check("three toasts show together", text().join(",") === "third,second,first");
  stack.show("fourth", "info");
  check("a fourth pushes the oldest out", text().join(",") === "fourth,third,second");
  mock.timers.tick(15000);
  check("the survivors still time out", toasts().length === 0);
}

// --- Lifetimes by severity ------------------------------------------------------

for (const [kind, lifetime] of [
  ["info", 15000],
  ["success", 15000],
  ["warning", 18000],
  ["error", 20000],
]) {
  const { stack, toasts } = mount();
  stack.show("timed", kind);
  mock.timers.tick(lifetime - 1);
  check(`a ${kind} toast stays for ${lifetime - 1}ms`, toasts().length === 1);
  mock.timers.tick(1);
  check(`a ${kind} toast leaves at ${lifetime}ms`, toasts().length === 0);
}

// --- Hover pauses the timer, leaving resumes the remainder -----------------------

{
  const { stack, toasts } = mount();
  stack.show("hovered", "info");
  const [toast] = toasts();
  mock.timers.tick(5000);
  hover(toast, "mouseenter");
  mock.timers.tick(60000);
  check("a hovered toast outlives its lifetime", toasts().length === 1);
  hover(toast, "mouseleave");
  mock.timers.tick(9999);
  check("leaving resumes the remaining 10s, not a fresh 15s", toasts().length === 1);
  mock.timers.tick(1);
  check("the toast leaves when the remainder runs out", toasts().length === 0);
}

{
  // A second hover pauses the remainder again, and keyboard focus pauses too.
  const { stack, toasts } = mount();
  stack.show("twice", "warning");
  const [toast] = toasts();
  mock.timers.tick(8000);
  hover(toast, "mouseenter");
  hover(toast, "mouseleave");
  mock.timers.tick(4000);
  hover(toast, "mouseenter");
  mock.timers.tick(30000);
  hover(toast, "mouseleave");
  mock.timers.tick(5999);
  check("two hovers spend only the time between them", toasts().length === 1);
  mock.timers.tick(1);
  check("the remainder after two pauses is 18s - 12s", toasts().length === 0);

  const { stack: focusStack, toasts: focusToasts } = mount();
  focusStack.show("focused", "error");
  const [focused] = focusToasts();
  focused.dispatchEvent(new window.FocusEvent("focusin", { bubbles: true }));
  mock.timers.tick(60000);
  check("a focused toast waits", focusToasts().length === 1);
  focused.dispatchEvent(new window.FocusEvent("focusout", { bubbles: true }));
  mock.timers.tick(20000);
  check("a toast resumes when focus leaves", focusToasts().length === 0);
}

// --- The close X ------------------------------------------------------------------

{
  const { stack, toasts } = mount();
  stack.show("keep", "info");
  stack.show("close me", "error");
  const closing = toasts()[0];
  closing.querySelector(".toast__close").click();
  check("the close X removes its toast at once", toasts().length === 1);
  check("the other toast stays", toasts()[0].querySelector(".toast__message").textContent === "keep");
  closing.querySelector(".toast__close").click();
  mock.timers.tick(25000);
  check("a dismissed toast's timer never touches the stack again", toasts().length === 0);
}

mock.timers.reset();

if (failures.length > 0) {
  console.error(`shared-toast: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("shared-toast: all assertions passed");
