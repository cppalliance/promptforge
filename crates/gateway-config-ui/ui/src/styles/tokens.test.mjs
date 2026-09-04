// Pins the CSS wiring end to end: the esbuild bundle must emit
// dist/app.css carrying the shared Cursor Dark design tokens
// (shared-ui/tokens.css) and the cascade layer order, and
// dist/index.html must link that stylesheet - a dropped import in
// main.ts or a dropped <link> would ship an unstyled UI without any
// build failure. Run after `npm run build` (a debug `cargo build` also
// produces dist/).
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const distDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "dist");

test("the bundled stylesheet defines the design tokens and layer order", async () => {
  const css = await readFile(path.join(distDir, "app.css"), "utf8");
  // \s* tolerates minified output; /i tolerates hex case changes.
  assert.match(css, /--cursor-base:\s*#F0F0F0/i, "the Cursor Dark primitive is defined");
  assert.match(css, /--accent:\s*#e4b570/i, "the PromptForge gold accent token is defined");
  assert.match(
    css,
    /--bg-primary:\s*var\(--cursor-editor\)/,
    "the page background aliases the Cursor editor surface",
  );
  assert.match(
    css,
    /@layer reset,\s*base,\s*components,\s*utilities/,
    "the cascade layer order is declared",
  );
  assert.match(
    css,
    /textarea\.input\s*\{[^}]*height:\s*auto[^}]*border-radius:\s*0?\.75rem/,
    "multiline inputs keep the rounded rectangle radius and natural height",
  );
  assert.match(
    css,
    /\.split-list :focus-visible[^}]*outline-offset:\s*-2px/,
    "focus rings stay inset inside scrollable panes",
  );
});

test("the shell page links the bundled stylesheet", async () => {
  const html = await readFile(path.join(distDir, "index.html"), "utf8");
  assert.match(
    html,
    /<link rel="stylesheet" href="app\.css">/,
    "index.html pulls in app.css",
  );
});
