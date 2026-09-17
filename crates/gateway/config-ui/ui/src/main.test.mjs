// Boots the bundled config UI (dist/index.html + dist/app.js) in jsdom
// and asserts the live shell's first paint: standalone with no stored
// key, the auto-boot on #app must land on the key prompt - medallion,
// title, labeled password input, and submit button - without touching
// the network. Run after `npm run build` (a debug `cargo build` also
// produces dist/).
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { JSDOM } from "jsdom";

const distDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "dist");

test("booting the bundle without a stored key renders the key prompt", async () => {
  const html = await readFile(path.join(distDir, "index.html"), "utf8");
  const dom = new JSDOM(html, { url: "http://127.0.0.1:8081/config/" });
  // The bundle reads the globals, not window properties.
  globalThis.window = dom.window;
  globalThis.document = dom.window.document;
  await import(pathToFileURL(path.join(distDir, "app.js")).href);

  const doc = dom.window.document;
  const title = doc.querySelector("#app .key-prompt h1");
  assert.ok(title, "the key prompt title mounted under #app");
  assert.equal(title.textContent, "PromptForge Gateway");

  const medallion = doc.querySelector("#app .key-prompt img");
  assert.ok(medallion, "the medallion is on the card");
  assert.equal(medallion.getAttribute("src"), "icons/promptforge-icon.png");
  assert.equal(
    medallion.getAttribute("srcset"),
    "icons/promptforge-icon.png 1x, icons/promptforge-icon@2x.png 2x",
    "the medallion names the @2x render for high-DPI displays",
  );
  assert.equal(medallion.getAttribute("alt"), "", "the medallion is decorative");
  assert.ok(medallion.getAttribute("width"), "the medallion reserves its width");
  assert.ok(medallion.getAttribute("height"), "the medallion reserves its height");

  const label = doc.querySelector("#app label[for='gateway-api-key']");
  assert.equal(label?.textContent, "API key", "the input has a real label");
  const input = doc.querySelector("#app #gateway-api-key");
  assert.equal(input?.getAttribute("type"), "password");

  const submit = doc.querySelector("#app button[type='submit']");
  assert.ok(submit, "the submit button is present");

  assert.equal(
    doc.querySelector("#app header.tab-bar"),
    null,
    "the shell stays unmounted until a key verifies",
  );
});
