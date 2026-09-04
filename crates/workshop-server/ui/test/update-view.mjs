// Unit test for the update view (src/ui/update-view.ts): the shared toast
// stack fires once when an update becomes available, a re-render in the
// same phase does not re-toast, a failed install toasts the error, and
// the install overlay carries the shared inline progress bar. Bundles the
// view with esbuild and drives it against jsdom with a stub-backend
// UpdateService and a recording toast stub.
// Run: node test/update-view.mjs.
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

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
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { UpdateService } from "./src/services/update-service.ts";
      export { UpdateView } from "./src/ui/update-view.ts";
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
  // The view and the shared progress bar import their colocated CSS; the
  // test drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});
const { lifecycle, UpdateService, UpdateView } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function toastStub() {
  const shown = [];
  return {
    shown,
    element: window.document.createElement("div"),
    show(message, kind) {
      shown.push({ message, kind });
    },
  };
}

function backendWith(update) {
  return {
    desktop: true,
    supported: async () => true,
    currentVersion: async () => "0.2.0",
    check: async () => update,
    relaunch: async () => undefined,
  };
}

async function waitForPhase(service, phase) {
  for (let i = 0; i < 100 && service.snapshot.phase !== phase; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

function updateNowButton() {
  return [...window.document.querySelectorAll(".update-banner button")].find(
    (button) => button.textContent === "Update now",
  );
}

await assertNoLeaks(lifecycle, async () => {
  // The happy path: available toasts once; the install's phases do not.
  const toasts = toastStub();
  const service = new UpdateService(
    backendWith({
      currentVersion: "0.2.0",
      version: "0.3.0",
      body: "Faster startup",
      async downloadAndInstall(onEvent) {
        onEvent({ event: "Started", data: { contentLength: 10 } });
        onEvent({ event: "Progress", data: { chunkLength: 10 } });
        onEvent({ event: "Finished" });
      },
      async close() {},
    }),
  );
  const view = new UpdateView(service, toasts);
  await service.checkNow();
  check(
    "an available update toasts once as info",
    toasts.shown.length === 1 &&
      toasts.shown[0]?.kind === "info" &&
      toasts.shown[0]?.message === "PromptForge 0.3.0 is available",
  );
  const banner = window.document.querySelector(".update-banner");
  check("the banner keeps the actionable available state", banner?.hidden === false);

  // Clicking Update now re-renders the same phase before the install's
  // first progress frame: the guard must not re-toast.
  updateNowButton()?.click();
  await waitForPhase(service, "restarting");
  check("the same phase never re-toasts", toasts.shown.length === 1);
  check("the install reached restarting", service.snapshot.phase === "restarting");
  const overlay = window.document.querySelector(".update-screen");
  check("the install overlay shows", overlay?.hidden === false);
  check(
    "the overlay carries the shared inline progress bar",
    overlay?.querySelector(".progress [class*='progress__fill'], .progress .progress__fill") !==
      null ||
      overlay?.querySelector(".progress")?.getAttribute("role") === "progressbar",
  );
  view.dispose();
  service.dispose();

  // The failure path: a failed install toasts the error.
  const failToasts = toastStub();
  const failService = new UpdateService(
    backendWith({
      currentVersion: "0.2.0",
      version: "0.3.0",
      body: "",
      async downloadAndInstall() {
        throw new Error("network down");
      },
      async close() {},
    }),
  );
  const failView = new UpdateView(failService, failToasts);
  await failService.checkNow();
  check(
    "the failed run's available phase toasts as info",
    failToasts.shown.length === 1 && failToasts.shown[0]?.kind === "info",
  );
  updateNowButton()?.click();
  await waitForPhase(failService, "error");
  check("the install reached the error phase", failService.snapshot.phase === "error");
  check(
    "a failed install toasts the error",
    failToasts.shown.length === 2 &&
      failToasts.shown[1]?.kind === "error" &&
      failToasts.shown[1]?.message === "Update failed: network down",
  );
  failView.dispose();
  failService.dispose();
});

if (failures.length > 0) {
  console.error(`update-view: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("update-view: all assertions passed");
