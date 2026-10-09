// Pins the tab bar: the right-side actions cluster shows the baked crate
// version (or the dev fallback) as a muted `vX.Y.Z` span, and the seven
// tabs are Cursor's settings nav - a sticky left column of grouped links
// that marks the current route, keeps a label for every icon, and
// collapses to a 40px icon-only column under 710px.
import assert from "node:assert/strict";
import test from "node:test";

import { bundledDeclarations, squash } from "../css-support.mjs";
import { bootApp, gatewayStub, modelsFixture, navigate, settle } from "../test-support.mjs";

const NARROW = "(max-width: 709.98px)";

async function openDesk() {
  const stub = gatewayStub({ key: "k", config: modelsFixture() });
  return bootApp({ key: "k", stub });
}

test("the tab bar shows the baked version in the actions cluster", async () => {
  const { root } = await openDesk();
  const label = root.querySelector(".tab-bar .tab-actions .tab-version");
  assert.ok(label, "the version label renders in the tab-actions cluster");
  assert.match(
    label.textContent,
    /^v(\d+\.\d+\.\d+|dev)$/,
    "the label shows the baked crate version (or the dev fallback)",
  );
});

test("the primary nav is a column beside the content, with a divider between each of three groups", async () => {
  const { root } = await openDesk();
  const nav = root.querySelector(".desk-layout > nav.tab-nav");
  assert.ok(nav, "the nav sits in the desk layout, not in the header strip");
  assert.equal(nav.getAttribute("aria-label"), "Primary");
  assert.ok(
    root.querySelector(".desk-layout > .desk-content > main#main"),
    "the content column holds main",
  );
  const sequence = [...nav.children].map((child) =>
    child.getAttribute("role") === "separator" ? "|" : child.textContent,
  );
  assert.deepEqual(
    sequence,
    ["Settings", "|", "Discover", "Local", "Remote", "Cloud", "|", "Profiles", "Secrets"],
    "dividers fall between groups, never at either end",
  );
});

test("the nav marks exactly the current route and follows the hash", async () => {
  const { dom, root } = await openDesk();
  for (const [hash, title] of [
    ["#/secrets", "Secrets"],
    ["#/cloud", "Cloud"],
    ["#/settings", "Settings"],
  ]) {
    navigate(dom, hash);
    await settle();
    const current = [...root.querySelectorAll(".tab-nav .tab[aria-current='page']")];
    assert.deepEqual(
      current.map((tab) => tab.textContent),
      [title],
      `${hash} marks only ${title}`,
    );
  }
});

test("every nav cell has a 12px icon and a label that survives the icon-only column", async () => {
  const { root } = await openDesk();
  const tabs = [...root.querySelectorAll(".tab-nav .tab")];
  assert.equal(tabs.length, 7);
  for (const tab of tabs) {
    const icon = tab.querySelector("svg");
    assert.ok(icon, `${tab.textContent} has an icon`);
    assert.equal(icon.getAttribute("width"), "12");
    assert.equal(icon.getAttribute("height"), "12");
    assert.equal(icon.getAttribute("aria-hidden"), "true");
    const label = tab.querySelector(".tab-label");
    assert.ok(label && label.textContent !== "", "the label stays in the DOM for assistive tech");
    assert.equal(tab.getAttribute("title"), label.textContent, "the icon-only column keeps a tooltip");
  }
});

test("the nav column is sticky and clamp-wide, 48px from the content, and icon-only under 710px", async () => {
  const layout = await bundledDeclarations(".desk-layout");
  assert.equal(layout.get("display"), "flex");
  assert.equal(squash(layout.get("gap")), "48px", "a 48px gap to the content");

  const nav = await bundledDeclarations(".tab-nav");
  assert.equal(nav.get("position"), "sticky");
  assert.equal(squash(nav.get("width")), "clamp(100px,25%,200px)");

  const cell = await bundledDeclarations(".tab");
  assert.equal(squash(cell.get("padding")), "4px6px", "cells are padded 4px 6px");
  assert.equal(cell.get("border-radius"), "var(--radius)", "a 6px radius");
  assert.equal(cell.get("font-size"), "var(--font-size-sm)", "12px text");
  assert.equal(cell.get("line-height"), "var(--line-height-sm)", "16px lines");
  assert.equal(cell.get("color"), "var(--cursor-text-secondary)");
  const hover = await bundledDeclarations(".tab:hover");
  const selected = await bundledDeclarations(".tab[aria-current=page]");
  assert.equal(hover.get("background"), "var(--cursor-bg-quaternary)", "6% hover fill");
  assert.equal(selected.get("background"), "var(--cursor-bg-quaternary)", "6% selected fill");

  const divider = await bundledDeclarations(".tab-divider");
  assert.equal(squash(divider.get("border-top")), "1pxsolidvar(--cursor-stroke-tertiary)");

  const narrow = await bundledDeclarations(".tab-nav", { media: NARROW });
  assert.equal(narrow.get("width"), "40px", "a 40px column");
  const label = await bundledDeclarations(".tab-label", { media: NARROW });
  assert.equal(squash(label.get("clip-path")), "inset(50%)", "the label hides but stays readable");
});
