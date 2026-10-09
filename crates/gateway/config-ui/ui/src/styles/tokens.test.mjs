// Pins the CSS wiring end to end: the esbuild bundle must emit
// dist/app.css containing the shared Cursor Dark design tokens
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

import { bundledDeclarations, bundledRuleCount, squash } from "../css-support.mjs";

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

test("the desk page links the bundled stylesheet", async () => {
  const html = await readFile(path.join(distDir, "index.html"), "utf8");
  assert.match(
    html,
    /<link rel="stylesheet" href="app\.css">/,
    "index.html pulls in app.css",
  );
});

// The gateway sheet carries the same IDE values as @workshop/look, because
// the two UIs are copies of one Cursor look. Hex literals compare without
// case; a value written as a formula compares as written.
const IDE_TOKENS = {
  "--cursor-red": "#E34671",
  "--cursor-blue": "#81A1C1",
  "--cursor-cyan": "#88C0D0",
  "--cursor-accent": "#81A1C1",
  "--cursor-sidebar": "#141414",
  "--cursor-shadow-primary": "#00000066",
  "--cursor-bg-active": "#F0F0F01E",
  "--cursor-bg-focused": "#F0F0F01E",
  "--cursor-text-link": "#81A1C1",
  "--cursor-text-invert": "#181818",
  "--radius-xs": "2px",
};

test("the bundled stylesheet carries the IDE token values", async () => {
  const css = await readFile(path.join(distDir, "app.css"), "utf8");
  for (const [name, value] of Object.entries(IDE_TOKENS)) {
    assert.match(css, new RegExp(`${name}:\\s*${value}\\s*;`, "i"), `${name} is ${value}`);
  }
  // The minifier writes 300ms as .3s, so the duration is pinned by value.
  assert.match(css, /--duration-slower:\s*(300ms|\.3s)\s*;/, "the slow transition is 300ms");
  assert.match(
    css,
    /--cursor-shadow-secondary:\s*color-mix\(in srgb,\s*var\(--cursor-shadow-primary\) 60%,\s*transparent\)/,
    "the secondary shadow ink is 60% of the widget shadow",
  );
  assert.match(
    css,
    /--cursor-shadow-tertiary:\s*color-mix\(in srgb,\s*var\(--cursor-shadow-primary\) 30%,\s*transparent\)/,
    "the tertiary shadow ink is 30% of the widget shadow",
  );
  assert.match(
    css,
    /--cursor-accent-hover:\s*color-mix\(in srgb,\s*white 10%,\s*var\(--cursor-accent\)\)/,
    "the accent hover mixes 10% white into the accent",
  );
  assert.match(
    css,
    /--code-font:\s*Consolas, Menlo, Monaco, "Droid Sans Mono", "Courier New", monospace/,
    "the code font is Cursor's stack",
  );
});

test("the tertiary and quaternary text names are the Cursor tiers, not swapped literals", async () => {
  const css = await readFile(path.join(distDir, "app.css"), "utf8");
  assert.match(css, /--text-tertiary:\s*var\(--cursor-text-tertiary\)\s*;/);
  assert.match(css, /--text-quaternary:\s*var\(--cursor-text-quaternary\)\s*;/);
  assert.match(css, /--cursor-text-tertiary:\s*color-mix\(in srgb,\s*var\(--cursor-base\) 60%/);
  assert.match(css, /--cursor-text-quaternary:\s*color-mix\(in srgb,\s*var\(--cursor-base\) 36%/);
});

test("buttons take Cursor's 24px control values and the accent primary", async () => {
  const button = await bundledDeclarations(".button");
  assert.equal(button.get("height"), "var(--height-sm)", "24px tall");
  assert.equal(button.get("padding-inline"), "var(--space-2)", "8px padding");
  assert.equal(button.get("border-radius"), "var(--radius)", "a 6px radius");
  assert.equal(button.get("font-size"), "var(--font-size-base)", "13px");
  assert.equal(button.get("font-weight"), "400");
  const small = await bundledDeclarations(".button-sm");
  assert.equal(small.get("height"), "var(--height-xs)", "20px tall");
  assert.equal(small.get("border-radius"), "var(--radius-sm)", "a 4px radius");
  const primary = await bundledDeclarations(".button-primary");
  assert.equal(primary.get("background"), "var(--cursor-accent)", "the accent replaces the gold");
  assert.equal(primary.get("color"), "var(--cursor-text-invert)");
  const hover = await bundledDeclarations(".button-primary:hover");
  assert.equal(hover.get("background"), "var(--cursor-accent-hover)");
  const secondary = await bundledDeclarations(".button-secondary");
  const secondaryHover = await bundledDeclarations(".button-secondary:hover");
  assert.equal(secondary.get("background"), "var(--cursor-bg-tertiary)", "an 8% fill");
  assert.equal(secondaryHover.get("background"), "var(--cursor-bg-secondary)", "a 14% hover");
});

test("the switch is the single shared 32x20 track with a 16px thumb that moves 12px", async () => {
  assert.equal(await bundledRuleCount(".switch"), 1, "one .switch rule: the gateway keeps no copy");
  const track = await bundledDeclarations(".switch");
  assert.equal(track.get("width"), "32px");
  assert.equal(track.get("height"), "20px");
  assert.equal(track.get("background"), "var(--cursor-bg-secondary)", "14% white when off");
  assert.equal(squash(track.get("transition")), "background-colorvar(--duration-slow)ease");
  const on = await bundledDeclarations(".switch[aria-checked=true]");
  assert.equal(on.get("background"), "var(--cursor-green)");
  const thumb = await bundledDeclarations(".switch::after");
  assert.equal(thumb.get("width"), "16px");
  assert.equal(thumb.get("height"), "16px");
  assert.match(thumb.get("background"), /^(white|#fff|#ffffff)$/, "a white thumb");
  const moved = await bundledDeclarations(".switch[aria-checked=true]::after");
  assert.match(moved.get("transform"), /^translateX?\(12px\)$/, "the thumb moves 12px");
});

test("modals take the form modal values", async () => {
  const overlay = await bundledDeclarations(".modal-overlay");
  assert.match(squash(overlay.get("background")), /^(rgba\(0,0,0,\.5\)|#00000080)$/, "a 0.5 backdrop");
  const dialog = await bundledDeclarations(".modal-dialog");
  assert.equal(dialog.get("width"), "320px", "a 320px card");
  assert.equal(dialog.get("border-radius"), "var(--radius-xl)", "a 12px radius");
  assert.equal(squash(dialog.get("border")), "1pxsolidvar(--cursor-stroke-secondary)", "a 12% stroke");
  assert.equal(dialog.get("box-shadow"), "none");
  assert.match(dialog.get("animation") ?? "", /var\(--duration-slower\)/, "opens in 300ms");
  const title = await bundledDeclarations(".modal-dialog > h2");
  assert.equal(squash(title.get("padding")), "var(--space-3)var(--space-4)", "header 12px 16px");
  assert.equal(title.get("font-size"), "var(--font-size-lg)", "a 14px title");
  const actions = await bundledDeclarations(".modal-actions");
  assert.equal(actions.get("padding"), "10px", "footer padding 10px");
  assert.equal(squash(actions.get("border-top")), "1pxsolidvar(--cursor-stroke-tertiary)", "an 8% top border");
});

test("both modal families drop the scale-in under reduced motion", async () => {
  const reduced = { media: "(prefers-reduced-motion: reduce)" };
  for (const selector of [".modal-dialog", ".modal"]) {
    const open = await bundledDeclarations(selector);
    assert.match(open.get("animation") ?? "", /modal-form-open/, `${selector} opens with the scale-in`);
    const still = await bundledDeclarations(selector, reduced);
    assert.equal(still.get("animation"), "none", `${selector} has no animation under reduced motion`);
  }
});