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

test("booting the bundle renders the placeholder shell title", async () => {
  const html = await readFile(path.join(distDir, "index.html"), "utf8");
  const dom = new JSDOM(html, { url: "http://127.0.0.1:8081/config/" });
  // The bundle reads the globals, not window properties.
  globalThis.window = dom.window;
  globalThis.document = dom.window.document;
  await import(pathToFileURL(path.join(distDir, "app.js")).href);

  const title = dom.window.document.querySelector("#app .shell-title");
  assert.ok(title, "the shell title mounted under #app");
  assert.equal(title.textContent, "PromptForge Gateway Config");
});
