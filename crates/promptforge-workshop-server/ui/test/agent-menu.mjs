// The agent menu (src/ui/agent-menu.ts) in jsdom against a scripted
// delegate: discovered agents render as launch buttons; an empty
// discovery shows the empty note; clicking launches through the
// delegate and disables the buttons until an error frees them; a launch
// while the socket is down shows the local failure note; the list
// re-renders on every discovery push. Run: node test/agent-menu.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { Emitter } from "./src/base/event.ts";
      export { AgentMenu } from "./src/ui/agent-menu.ts";
    `,
    resolveDir: path.join(testDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The module under test imports its colocated CSS; strip it - the test
  // drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
});
const { window } = dom;
for (const key of ["document", "HTMLElement", "Node", "Element", "Event"]) {
  if (!(key in globalThis) && key in window) {
    globalThis[key] = window[key];
  }
}
globalThis.window = window;
globalThis.document = window.document;

const bundlePath = path.join(os.tmpdir(), "promptforge-agent-menu-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, Emitter, AgentMenu } = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// The scripted delegate: the AgentSessionService slice the menu reads.
function makeDelegate(agents = []) {
  const changed = new Emitter();
  const errors = new Emitter();
  return {
    agents,
    onDidChangeAgents: changed.event,
    onError: errors.event,
    launched: [],
    launchResult: true,
    launch(agent) {
      this.launched.push(agent);
      return this.launchResult;
    },
    push(list) {
      this.agents = list;
      changed.fire(list);
    },
    fail(message) {
      errors.fire(message);
    },
  };
}

const buttonsOf = (menu) => [...menu.element.querySelectorAll(".agent-menu__launch")];
const emptyOf = (menu) => menu.element.querySelector(".agent-menu__empty");
const errorOf = (menu) => menu.element.querySelector(".agent-menu__error");

await assertNoLeaks(lifecycle, () => {
  // --- Discovery renders as launch buttons; empty shows the note -----------

  {
    const delegate = makeDelegate([]);
    const menu = new AgentMenu(delegate);
    check("an empty discovery shows the empty note", emptyOf(menu).hidden === false);
    check("an empty discovery renders no buttons", buttonsOf(menu).length === 0);
    delegate.push(["chat", "research"]);
    check(
      "every discovered agent renders as a launch button, in order",
      isDeepStrictEqual(buttonsOf(menu).map((button) => button.textContent), [
        "chat",
        "research",
      ]),
    );
    check("a non-empty discovery hides the empty note", emptyOf(menu).hidden === true);
    delegate.push(["solo"]);
    check(
      "a later push re-renders the complete snapshot",
      isDeepStrictEqual(buttonsOf(menu).map((button) => button.textContent), ["solo"]),
    );
    menu.dispose();
  }

  // --- Launching dispatches, disables, and an error frees the menu ---------

  {
    const delegate = makeDelegate(["chat", "research"]);
    const menu = new AgentMenu(delegate);
    buttonsOf(menu)[0].click();
    check("clicking a button launches its agent", isDeepStrictEqual(delegate.launched, ["chat"]));
    check(
      "a sent launch disables the buttons until the server answers",
      buttonsOf(menu).every((button) => button.disabled),
    );
    buttonsOf(menu)[1].click();
    check("a disabled menu launches nothing more", isDeepStrictEqual(delegate.launched, ["chat"]));
    delegate.fail("unknown agent: chat");
    check("a refused launch shows the server's message", errorOf(menu).textContent === "unknown agent: chat");
    check("the error is visible", errorOf(menu).hidden === false);
    check(
      "the error frees the menu for another launch",
      buttonsOf(menu).every((button) => !button.disabled),
    );
    buttonsOf(menu)[1].click();
    check(
      "the freed menu launches again",
      isDeepStrictEqual(delegate.launched, ["chat", "research"]),
    );
    check("a new launch clears the stale error", errorOf(menu).hidden === true);
    menu.dispose();
  }

  // --- A launch while the socket is down shows the local note --------------

  {
    const delegate = makeDelegate(["chat"]);
    delegate.launchResult = false;
    const menu = new AgentMenu(delegate);
    buttonsOf(menu)[0].click();
    check("the failed send shows the local socket-down note", errorOf(menu).hidden === false);
    check(
      "a failed send leaves the menu enabled for the retry",
      buttonsOf(menu).every((button) => !button.disabled),
    );
    menu.dispose();
  }

  // --- Disposal severs the delegate subscriptions ---------------------------

  {
    const delegate = makeDelegate(["chat"]);
    const menu = new AgentMenu(delegate);
    menu.dispose();
    delegate.push(["chat", "late"]);
    check(
      "a disposed menu stops re-rendering",
      buttonsOf(menu).length === 1,
    );
  }
});

if (failures.length > 0) {
  console.error(`agent-menu: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("agent-menu: all assertions passed");
process.exit(0);
