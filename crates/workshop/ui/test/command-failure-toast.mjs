// A command that throws or rejects raises Cursor's toast, "Command '{0}'
// resulted in an error", with {0} the command's palette label (Category:
// Title, the bare title, or the id). The platform registry reports the
// failure (platform/test/command-failures.mjs); toastCommandFailures turns
// each report into an error toast on the shared stack, and disposing it
// stops that. The toast carries no error text, so the detail stays with
// the caller's console log. A run that bypasses the registry (the layout
// core's direct close) raises the same toast by hand. Bundles the module with
// esbuild and drives it against jsdom with the real toast stack.
// Run: node --test test/command-failure-toast.mjs
import { readFile } from "node:fs/promises";
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
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { toastCommandFailures, toastCommandFailure } from "./src/services/command-failure-toasts.ts";
      export { CommandRegistry } from "@workshop/platform/command-registry";
      export { createToastStack } from "@workshop/look/toast";
      export { registerService } from "@workshop/platform/service-registry";
      export { TOAST_STACK } from "./src/services/toast-service.ts";
    `,
    resolveDir: path.join(uiDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  loader: { ".css": "empty" },
});
const { toastCommandFailures, toastCommandFailure, CommandRegistry, createToastStack, registerService, TOAST_STACK } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const stack = createToastStack();
window.document.body.append(stack.element);
const toasts = () =>
  [...stack.element.querySelectorAll(".toast")].map((toast) => ({
    text: toast.querySelector(".toast__message").textContent,
    error: toast.classList.contains("toast-error"),
  }));

const commands = new CommandRegistry();
const watch = toastCommandFailures(commands, stack);
commands.register("file.save", { title: "Save", category: "File", run: () => Promise.reject(new Error("disk full")) });
commands.register("window.reload", {
  title: "Reload Window",
  run: () => {
    throw new TypeError("not a function");
  },
});
commands.register("fine", { title: "Fine", run: () => {} });

await commands.execute("file.save").catch(() => undefined);
check(
  "a rejected command raises the toast with its Category: Title label",
  toasts().length === 1 && toasts()[0].text === "Command 'File: Save' resulted in an error",
);
check("the toast is an error toast", toasts()[0]?.error === true);
check("the toast leaves the error text out", !toasts()[0]?.text.includes("disk full"));

await commands.execute("window.reload").catch(() => undefined);
check(
  "a synchronous throw raises the toast with the bare title",
  toasts()[0]?.text === "Command 'Reload Window' resulted in an error" && toasts().length === 2,
);

await commands.execute("fine");
await commands.execute("missing");
check("a command that succeeds and an unknown id raise nothing", toasts().length === 2);

watch.dispose();
await commands.execute("file.save").catch(() => undefined);
check("a disposed subscription raises no more toasts", toasts().length === 2);

// A run that bypassed the registry (the layout core's direct close) reports by hand.
const logged = [];
const realConsoleError = console.error;
console.error = (...args) => logged.push(args);
toastCommandFailure("file.save", new Error("disk full"), commands);
check("a bare widget with no stack registered raises nothing", toasts().length === 2);
registerService(TOAST_STACK, () => stack);
toastCommandFailure("file.save", new Error("disk full"), commands);
console.error = realConsoleError;
check(
  "a direct failure raises the same toast, labelled from the registry",
  toasts().length === 3 && toasts()[0].text === "Command 'File: Save' resulted in an error" && toasts()[0].error,
);
check(
  "a direct failure keeps its detail on the console both times",
  logged.length === 2 && logged.every((args) => String(args[0]).includes("file.save") && args[1]?.message === "disk full"),
);

// The composition root wires the shared registry to the shared stack, right
// after it registers the stack. A booted app cannot reach the bundle's
// registry, so the wiring is read from the source.
const main = await readFile(path.join(uiDir, "..", "src", "main.ts"), "utf8");
check(
  "main.ts toasts the shared registry's failures on the shared stack, once, after registering it",
  /registerService\(TOAST_STACK, \(\) => toasts\);\s*disposables\.add\(toastCommandFailures\(Commands, toasts\)\);/.test(main) &&
    main.match(/toastCommandFailures\(/g)?.length === 1,
);

if (failures.length > 0) {
  console.error(`command-failure-toast: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("command-failure-toast: all assertions passed");
process.exit(0);
