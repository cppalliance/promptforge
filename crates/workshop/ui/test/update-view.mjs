// Unit test for the update view (src/parts/chrome/update-view.ts): the shared toast
// stack fires once when an update becomes available, a re-render in the
// same phase does not re-toast, a failed install toasts the error, a
// failed startup check stays quiet while a failed check the user asks for
// toasts, and the install overlay shows the shared inline progress bar.
// The banner's button and the overlay's status lines wear Cursor's
// update-button labels (Download Update, Downloading Update...,
// Installing Update...). The About dialog (src/parts/chrome/about-dialog.ts)
// shares the file because it drives the same UpdateService: a 380px card with
// the product icon and a copy button beside the version, and beside the date
// once a build defines one (a bundle without the define has no date row), a
// "Copy version info" primary button that Enter activates, Escape closing
// the card, and the check button's Cursor labels (Check for Updates...,
// Checking for Updates..., Download Update). Every copy confirmation (a
// row's check icon, the primary button's "Copied") shows only when the
// clipboard write landed: a refused write and a missing clipboard confirm
// nothing.
// Bundles the view with esbuild and drives it against jsdom with a
// stub-backend UpdateService and a recording toast stub.
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
      export * as lifecycle from "@workshop/platform/lifecycle";
      export { UpdateService } from "./src/services/update-service.ts";
      export { UpdateView } from "./src/parts/chrome/update-view.ts";
      export { showAboutDialog } from "./src/parts/chrome/about-dialog.ts";
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
const { lifecycle, UpdateService, UpdateView, showAboutDialog } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

// A second About bundle with a build date defined, the way a build that
// supplies __APP_BUILD_DATE__ would ship it.
const datedBundle = await esbuild.build({
  stdin: {
    contents: `export { showAboutDialog } from "./src/parts/chrome/about-dialog.ts";`,
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
  define: { __APP_BUILD_DATE__: JSON.stringify("2026-10-08") },
});
const { showAboutDialog: showDatedAboutDialog } = await import(
  `data:text/javascript;base64,${Buffer.from(datedBundle.outputFiles[0].text).toString("base64")}`
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
  return [...window.document.querySelectorAll(".ws-update-banner button")].find(
    (button) => button.textContent === "Download Update",
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
  const banner = window.document.querySelector(".ws-update-banner");
  check("the banner keeps the actionable available state", banner?.hidden === false);

  // Clicking Download Update re-renders the same phase before the install's
  // first progress frame: the guard must not re-toast.
  updateNowButton()?.click();
  await waitForPhase(service, "restarting");
  check("the same phase never re-toasts", toasts.shown.length === 1);
  check("the install reached restarting", service.snapshot.phase === "restarting");
  const overlay = window.document.querySelector(".ws-update-screen");
  check("the install overlay shows", overlay?.hidden === false);
  check(
    "the overlay shows the shared inline progress bar",
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

  // A failed startup check stays quiet; the same failure from a check the
  // user asks for toasts.
  const quietToasts = toastStub();
  const quietService = new UpdateService({
    ...backendWith(null),
    check: async () => {
      throw new Error("Could not fetch a valid release JSON from the remote");
    },
  });
  const quietView = new UpdateView(quietService, quietToasts);
  quietService.startAutoCheck(0);
  await waitForPhase(quietService, "error");
  check("the startup check reached the error phase", quietService.snapshot.phase === "error");
  check("a failed startup check shows no toast", quietToasts.shown.length === 0);
  await quietService.checkNow();
  check(
    "a failed check the user asks for toasts the error",
    quietToasts.shown.length === 1 &&
      quietToasts.shown[0]?.kind === "error" &&
      quietToasts.shown[0]?.message ===
        "Update failed: Could not fetch a valid release JSON from the remote",
  );
  quietView.dispose();
  quietService.dispose();

  // An install started from the startup check's banner is the user's
  // request, so its failure still toasts.
  const installToasts = toastStub();
  const installService = new UpdateService(
    backendWith({
      currentVersion: "0.2.0",
      version: "0.3.0",
      body: "",
      async downloadAndInstall() {
        throw new Error("disk full");
      },
      async close() {},
    }),
  );
  const installView = new UpdateView(installService, installToasts);
  installService.startAutoCheck(0);
  await waitForPhase(installService, "available");
  updateNowButton()?.click();
  await waitForPhase(installService, "error");
  check(
    "a failed install after the startup check toasts the error",
    installToasts.shown.length === 2 &&
      installToasts.shown[1]?.kind === "error" &&
      installToasts.shown[1]?.message === "Update failed: disk full",
  );
  installView.dispose();
  installService.dispose();

  // The overlay's status lines wear Cursor's capitalization: a download
  // held open shows "Downloading Update... 50%".
  let releaseDownload = () => {};
  const gate = new Promise((resolve) => {
    releaseDownload = resolve;
  });
  const slowService = new UpdateService(
    backendWith({
      currentVersion: "0.2.0",
      version: "0.3.0",
      body: "",
      async downloadAndInstall(onEvent) {
        onEvent({ event: "Started", data: { contentLength: 10 } });
        onEvent({ event: "Progress", data: { chunkLength: 5 } });
        await gate;
        onEvent({ event: "Finished" });
      },
      async close() {},
    }),
  );
  const slowView = new UpdateView(slowService, toastStub());
  await slowService.checkNow();
  updateNowButton()?.click();
  await waitForPhase(slowService, "downloading");
  check(
    "a download in flight reads Downloading Update... with its percentage",
    window.document.querySelector(".ws-update-screen__panel p")?.textContent === "Downloading Update... 50%",
  );
  releaseDownload();
  await waitForPhase(slowService, "restarting");
  slowView.dispose();
  slowService.dispose();

  // --- The About dialog -----------------------------------------------------

  const clipboardWrites = [];
  let refuseWrites = false;
  const fakeNavigator = {
    clipboard: {
      writeText: async (text) => {
        if (refuseWrites) {
          throw new Error("NotAllowedError: clipboard write refused");
        }
        clipboardWrites.push(text);
      },
    },
  };
  const originalNavigator = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  Object.defineProperty(globalThis, "navigator", {
    value: fakeNavigator,
    configurable: true,
    writable: true,
  });
  const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
  const aboutService = new UpdateService(backendWith(null));
  await aboutService.checkNow();
  const invoker = window.document.createElement("button");
  window.document.body.appendChild(invoker);
  invoker.focus();
  const about = showAboutDialog(aboutService);
  const card = window.document.querySelector(".ws-about-dialog");
  check(
    "the About card is a modal dialog named by its title",
    card?.getAttribute("role") === "dialog" &&
      card?.getAttribute("aria-modal") === "true" &&
      card?.getAttribute("aria-labelledby") === card?.querySelector("h2")?.id,
  );
  const icon = card?.querySelector(".ws-about-dialog__icon");
  check(
    "the About card shows the decorative product icon",
    icon?.tagName === "IMG" && icon?.getAttribute("src") === "/icons/promptforge-icon.png" && icon?.getAttribute("alt") === "",
  );
  const field = (name) => card?.querySelector(`.ws-about-dialog__row[data-field="${name}"]`);
  check("the version row shows the running version", field("version")?.querySelector(".ws-about-dialog__value")?.textContent === "0.2.0");
  check(
    "a bundle without a build date shows no date row, not an Unknown one",
    field("date") === null && !card?.textContent?.includes("Unknown"),
  );
  const copyVersion = field("version")?.querySelector("button");
  check("the version row has a labelled copy button", copyVersion?.getAttribute("aria-label") === "Copy version");
  const copyIconHtml = copyVersion?.innerHTML;
  copyVersion?.click();
  await flush();
  check("the version's copy button copies the version", clipboardWrites.at(-1) === "0.2.0");
  check("a landed copy swaps the row's icon to the check", copyVersion?.innerHTML !== copyIconHtml);

  const primary = card?.querySelector(".ws-about-dialog__primary");
  check("the primary button reads Copy version info", primary?.textContent === "Copy version info");
  check("the primary button holds focus on open", window.document.activeElement === primary);
  const pressKey = (key) => {
    const event = new window.KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
    window.document.dispatchEvent(event);
    return event;
  };
  clipboardWrites.length = 0;
  const enter = pressKey("Enter");
  await flush();
  check(
    "Enter copies the version info",
    clipboardWrites.length === 1 && clipboardWrites[0] === "Version: 0.2.0",
  );
  check("Enter's press is consumed so the focused button does not click a second time", enter.defaultPrevented === true);
  check("a copied primary button says so", primary?.textContent === "Copied");

  const closeButton = card?.querySelector(".ws-about-dialog__close");
  closeButton?.focus();
  clipboardWrites.length = 0;
  const enterOnClose = pressKey("Enter");
  await flush();
  check(
    "Enter on another button leaves that button's own action alone",
    clipboardWrites.length === 0 && enterOnClose.defaultPrevented === false,
  );

  const checkButton = card?.querySelector(".ws-about-dialog__check");
  check("the check button wears Cursor's idle label", checkButton?.textContent === "Check for Updates...");
  pressKey("Escape");
  check("Escape closes the About card", window.document.querySelector(".ws-about-dialog") === null);
  check("closing the About card returns focus to its invoker", window.document.activeElement === invoker);
  about.dispose();

  // A copy confirmation shows only when the write landed: a refused write
  // and a missing clipboard leave the icon and the primary label alone, and
  // the next landed write confirms.
  {
    const refusedAbout = showAboutDialog(aboutService);
    const refusedCard = window.document.querySelector(".ws-about-dialog");
    const rowCopy = refusedCard?.querySelector('.ws-about-dialog__row[data-field="version"] button');
    const rowIcon = rowCopy?.innerHTML;
    const refusedPrimary = refusedCard?.querySelector(".ws-about-dialog__primary");
    clipboardWrites.length = 0;

    refuseWrites = true;
    rowCopy?.click();
    refusedPrimary?.click();
    await flush();
    check("a refused write leaves the row's copy icon alone", rowCopy?.innerHTML === rowIcon);
    check(
      "a refused write leaves the primary button reading Copy version info",
      refusedPrimary?.textContent === "Copy version info",
    );

    refuseWrites = false;
    fakeNavigator.clipboard = undefined;
    rowCopy?.click();
    refusedPrimary?.click();
    await flush();
    check("a missing clipboard leaves the row's copy icon alone", rowCopy?.innerHTML === rowIcon);
    check(
      "a missing clipboard leaves the primary button reading Copy version info",
      refusedPrimary?.textContent === "Copy version info",
    );

    fakeNavigator.clipboard = {
      writeText: async (text) => {
        clipboardWrites.push(text);
      },
    };
    rowCopy?.click();
    refusedPrimary?.click();
    await flush();
    check("a landed write swaps the row's icon to the check", rowCopy?.innerHTML !== rowIcon);
    check("a landed write labels the primary button Copied", refusedPrimary?.textContent === "Copied");
    check(
      "only the landed writes reached the clipboard",
      clipboardWrites.length === 2 &&
        clipboardWrites[0] === "0.2.0" &&
        clipboardWrites[1] === "Version: 0.2.0",
    );
    refusedAbout.dispose();
  }

  // A build that defines the date gets the date row, its copy button, and
  // the Date line in the primary copy.
  {
    const datedAbout = showDatedAboutDialog(aboutService);
    const datedCard = window.document.querySelector(".ws-about-dialog");
    const dateRow = datedCard?.querySelector('.ws-about-dialog__row[data-field="date"]');
    check(
      "a build date appears as a labelled row after the version",
      dateRow?.querySelector(".ws-about-dialog__value")?.textContent === "2026-10-08" &&
        dateRow?.previousElementSibling?.getAttribute("data-field") === "version",
    );
    const dateCopy = dateRow?.querySelector("button");
    check("the date row has a labelled copy button", dateCopy?.getAttribute("aria-label") === "Copy date");
    const dateIcon = dateCopy?.innerHTML;
    clipboardWrites.length = 0;
    dateCopy?.click();
    await flush();
    check(
      "the date's copy button copies the date and confirms",
      clipboardWrites.length === 1 && clipboardWrites[0] === "2026-10-08" && dateCopy?.innerHTML !== dateIcon,
    );
    clipboardWrites.length = 0;
    datedCard?.querySelector(".ws-about-dialog__primary")?.click();
    await flush();
    check(
      "the primary copy carries the version and the date",
      clipboardWrites.length === 1 && clipboardWrites[0] === "Version: 0.2.0\nDate: 2026-10-08",
    );
    datedAbout.dispose();
  }

  // The check button follows the phase with Cursor's labels.
  const availableService = new UpdateService(
    backendWith({
      currentVersion: "0.2.0",
      version: "0.3.0",
      body: "",
      async downloadAndInstall() {},
      async close() {},
    }),
  );
  await availableService.checkNow();
  const availableAbout = showAboutDialog(availableService);
  check(
    "an available update labels the check button Download Update",
    window.document.querySelector(".ws-about-dialog__check")?.textContent === "Download Update",
  );
  availableAbout.dispose();
  availableService.dispose();
  const checkingService = new UpdateService({
    ...backendWith(null),
    check: () => new Promise(() => {}),
  });
  void checkingService.checkNow();
  await flush();
  const checkingAbout = showAboutDialog(checkingService);
  const checking = window.document.querySelector(".ws-about-dialog__check");
  check(
    "a check in flight reads Checking for Updates... and is disabled",
    checking?.textContent === "Checking for Updates..." && checking?.disabled === true,
  );
  checkingAbout.dispose();
  checkingService.dispose();
  aboutService.dispose();
  invoker.remove();
  if (originalNavigator !== undefined) {
    Object.defineProperty(globalThis, "navigator", originalNavigator);
  }
});

if (failures.length > 0) {
  console.error(`update-view: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("update-view: all assertions passed");
