// Boots the bundled config UI (dist/index.html + dist/app.js) in jsdom
// and asserts the placeholder shell renders - the same bundle-level
// harness the workshop UI tests use. Run after `npm run build` (a debug
// `cargo build` also produces dist/).
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { JSDOM } from "jsdom";

const distDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "dist");

test("booting the bundle renders the shell: tab bar, primary nav, title", async () => {
  const html = await readFile(path.join(distDir, "index.html"), "utf8");
  const dom = new JSDOM(html, { url: "http://127.0.0.1:8081/config/" });
  // The bundle reads the globals, not window properties.
  globalThis.window = dom.window;
  globalThis.document = dom.window.document;
  await import(pathToFileURL(path.join(distDir, "app.js")).href);

  const title = dom.window.document.querySelector("#app main .shell-title");
  assert.ok(title, "the shell title mounted inside <main> under #app");
  assert.equal(title.textContent, "PromptForge Gateway Config");

  const tabs = dom.window.document.querySelectorAll(
    "#app header.tab-bar nav[aria-label='Primary'] a.tab",
  );
  assert.equal(tabs.length, 6, "the tab bar holds the six destinations");
  const active = dom.window.document.querySelectorAll("#app .tab[aria-current='page']");
  assert.equal(active.length, 1, "exactly one tab is marked current");
  assert.equal(active[0].textContent, "Models");

  const skip = dom.window.document.querySelector("#app > a.skip-link:first-child");
  assert.ok(skip, "the skip link is the first element in the shell");
  const main = dom.window.document.querySelector("#app > main#main");
  assert.ok(main, "the main region carries the skip target id");
  skip.click();
  assert.equal(
    dom.window.document.activeElement,
    main,
    "activating the skip link focuses the main region",
  );
  assert.equal(
    dom.window.location.hash,
    "",
    "the skip jump never rewrites the route hash",
  );
});
