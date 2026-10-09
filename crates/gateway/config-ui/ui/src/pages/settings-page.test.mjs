// Pins the Settings view's frame and System panel: the seven-section
// nav column with hash routing and the system default, the metric tiles
// rendered from GET /admin/system (vendor chip color by GPU name, the
// VRAM card hidden without a GPU), the Storage card, the About panel,
// and the 5s poll that runs only while the System panel is mounted.
import assert from "node:assert/strict";
import test from "node:test";

import { bundledDeclarations, squash } from "../css-support.mjs";
import {
  GIB,
  bootApp,
  gatewayStub,
  modelsFixture,
  navigate,
  settle,
  systemFixture,
} from "../test-support.mjs";

function fixtureStub(extra = {}) {
  return gatewayStub({
    key: "k",
    config: modelsFixture(),
    models: ["qwen-common"],
    ...extra,
  });
}

function systemCalls(stub) {
  return stub.calls.filter((call) => call.url.endsWith("/admin/system")).length;
}

test("the nav renders seven sections, routes by hash, and defaults to System", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings");
  await settle();
  const links = [...root.querySelectorAll(".settings-nav-link")];
  assert.deepEqual(
    links.map((link) => link.textContent),
    ["System", "Gateway", "Workshop", "Dominions", "Endpoints", "Tools", "About"],
    "the nav column lists the seven sections in order",
  );
  assert.equal(
    root.querySelector(".settings-panel").dataset.section,
    "system",
    "a bare #/settings lands on the System panel",
  );
  assert.equal(
    root.querySelector(".settings-nav-link[aria-current='true']").textContent,
    "System",
  );

  for (const section of ["gateway", "workshop", "dominions", "endpoints", "tools", "about"]) {
    navigate(dom, `#/settings/${section}`);
    await settle();
    assert.equal(
      root.querySelector(".settings-panel").dataset.section,
      section,
      `#/settings/${section} mounts its panel`,
    );
  }
});

test("the System metric tiles render from the snapshot with the vendor chip", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/system");
  await settle();

  const cpu = root.querySelector(".metric-cpu");
  assert.match(cpu.querySelector(".metric-value").textContent, /2\.50 GHz/);
  assert.match(cpu.querySelector(".metric-sub").textContent, /16 logical \/ 8 physical/);
  assert.ok(cpu.querySelector(".progress__fill"), "the CPU tile shows a utilization bar");

  const ram = root.querySelector(".metric-ram");
  assert.match(ram.querySelector(".metric-value").textContent, /32\.0 \/ 64\.0 GiB/);

  const vram = root.querySelector(".metric-vram");
  assert.match(vram.querySelector(".metric-value").textContent, /4\.0 \/ 24\.0 GiB/);
  assert.ok(
    vram.querySelector(".metric-bar-segmented"),
    "the VRAM bar is the segmented variant",
  );
  assert.match(vram.querySelector(".gpu-name").textContent, /NVIDIA GeForce RTX 4090/);
  const chip = vram.querySelector(".vendor-chip");
  assert.equal(chip.dataset.vendor, "NVIDIA");
  assert.equal(chip.style.color, "rgb(118, 185, 0)", "the NVIDIA chip is #76B900");

  const disk = root.querySelector(".metric-disk");
  assert.match(
    disk.querySelector(".metric-value").textContent,
    /700\u00a0GiB \/ 4\.0\u00a0TiB/,
  );
  assert.match(disk.querySelector(".metric-sub").textContent, /C:\/pf\/cache/);

  assert.ok(root.querySelector(".gpu-devices"), "the GPU Devices section renders");
  assert.match(
    root.querySelector(".gpu-vram-pill").textContent,
    /4\.0 \/ 24\.0 GiB/,
    "the device row repeats the VRAM readings",
  );

  const storage = root.querySelector(".settings-card .field-row[data-key='cache_dir']");
  assert.equal(storage.querySelector("input").value, "~/.promptforge");
  assert.match(
    root.querySelector(".storage-warning").textContent,
    /does not move existing files/,
  );
});

test("an AMD GPU name colors the vendor chip red", async () => {
  const system = systemFixture();
  system.gpu = { name: "AMD Radeon RX 7900 XTX", vram_used_bytes: GIB, vram_total_bytes: 24 * GIB };
  const stub = fixtureStub({ system });
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/system");
  await settle();
  const chip = root.querySelector(".vendor-chip");
  assert.equal(chip.dataset.vendor, "AMD");
  assert.equal(chip.style.color, "rgb(237, 28, 36)", "the AMD chip is #ED1C24");
});

test("a machine without a GPU hides the VRAM card and the devices section", async () => {
  const system = systemFixture();
  system.gpu = null;
  const stub = fixtureStub({ system });
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/system");
  await settle();
  assert.ok(root.querySelector(".metric-cpu"), "the CPU tile still renders");
  assert.equal(root.querySelector(".metric-vram"), null, "no VRAM tile without a GPU");
  assert.equal(root.querySelector(".vendor-chip"), null, "no vendor chip without a GPU");
  assert.equal(root.querySelector(".gpu-devices"), null, "no GPU Devices section");
});

test("the system poll runs every 5s while mounted and stops after unmount", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const stub = fixtureStub();
  const { dom } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/system");
  await settle();
  assert.equal(systemCalls(stub), 1, "mounting the System panel fetches once immediately");

  t.mock.timers.tick(5000);
  await settle();
  assert.equal(systemCalls(stub), 2, "the interval refreshes the snapshot after 5s");

  navigate(dom, "#/local");
  await settle();
  t.mock.timers.tick(5000);
  await settle();
  t.mock.timers.tick(5000);
  await settle();
  assert.equal(systemCalls(stub), 2, "no poll fires once the panel is unmounted");
});

test("the About panel renders the medallion, version, and license link", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/about");
  await settle();
  const medallion = root.querySelector("img.about-medallion");
  assert.ok(medallion, "the medallion renders");
  assert.equal(medallion.getAttribute("src"), "icons/promptforge-icon.png");
  assert.equal(
    medallion.getAttribute("srcset"),
    "icons/promptforge-icon.png 1x, icons/promptforge-icon@2x.png 2x",
    "the About medallion names the @2x render for high-DPI displays",
  );
  assert.match(root.querySelector(".about-name").textContent, /PromptForge Gateway/);
  assert.match(
    root.querySelector(".about-version").textContent,
    /^Version (\d+\.\d+\.\d+|dev)$/,
    "the version is the baked crate version (or the dev fallback)",
  );
  const license = root.querySelector(".about-license");
  assert.equal(license.href, "https://www.boost.org/LICENSE_1_0.txt");
  assert.equal(license.rel, "noopener");
});

test("a settings section puts its title above the card and lays each row as label, control, help", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/settings/system");
  await settle();

  const card = root.querySelector(".settings-card .field-row[data-key='cache_dir']").closest(".settings-card");
  const kids = [...card.children];
  const heading = card.querySelector(":scope > .section-heading");
  const body = card.querySelector(":scope > .section-body");
  assert.ok(heading && body, "the title and the card body are siblings");
  assert.ok(kids.indexOf(heading) < kids.indexOf(body), "the title sits above the card");
  assert.equal(body.contains(heading), false, "the title is not inside the card");

  const row = body.querySelector(".field-row[data-key='cache_dir']");
  const order = [...row.children].map((child) =>
    child.classList.contains("field-head")
      ? "label"
      : child.classList.contains("field-help")
        ? "help"
        : "control",
  );
  assert.deepEqual(order, ["label", "control", "help"], "the grid reads label, control, then help");
});

test("the card is a 4% tinted 12px block with no border or padding, and its rows are grid cells", async () => {
  const card = await bundledDeclarations(".settings-card");
  assert.equal(card.get("border"), "0", "no border on the wrapper");
  assert.equal(card.get("padding"), "0", "no padding on the wrapper");
  assert.equal(card.get("background"), "none");

  const heading = await bundledDeclarations(".settings-card .section-heading");
  assert.equal(heading.get("font-size"), "var(--font-size-sm)", "a 12px title");
  assert.equal(heading.get("color"), "var(--cursor-text-secondary)");

  const body = await bundledDeclarations(".settings-card > .section-body");
  assert.equal(body.get("background"), "var(--cursor-bg-quinary)", "a 4% tint");
  assert.equal(body.get("border-radius"), "var(--radius-xl)", "a 12px radius");
  assert.equal(body.get("padding"), "0");

  const row = await bundledDeclarations(".field-row");
  assert.equal(row.get("display"), "grid");
  const control = await bundledDeclarations(".field-row>:not(.field-head):not(.field-help)");
  assert.equal(control.get("grid-column"), "2", "the control sits on the right");
  const label = await bundledDeclarations(".field-head label");
  assert.equal(label.get("font-size"), "var(--font-size-base)", "a 13px label");
  assert.equal(label.get("font-weight"), "400");
  const help = await bundledDeclarations(".field-help");
  assert.equal(help.get("font-size"), "var(--font-size-base)", "13px help text");
  assert.equal(help.get("color"), "var(--cursor-text-secondary)");
});

test("banners are 12px cards, the pending one in the accent tint, and callouts carry a 4px left bar", async () => {
  const banner = await bundledDeclarations(".banner");
  assert.equal(banner.get("border-radius"), "var(--radius-xl)");
  assert.equal(squash(banner.get("padding")), "12px14px");
  const pending = await bundledDeclarations(".banner-pending");
  assert.equal(pending.get("background"), "var(--cursor-accent-subtle)", "accent at 8%");
  assert.equal(
    squash(pending.get("border-color")),
    "color-mix(insrgb,var(--cursor-accent)20%,transparent)",
    "accent at 20%",
  );
  for (const [selector, color] of [
    [".banner-restart", "var(--warning)"],
    [".banner-warning", "var(--warning)"],
    [".banner-danger", "var(--danger)"],
  ]) {
    const callout = await bundledDeclarations(selector);
    assert.equal(callout.get("border-left-width"), "4px", `${selector} has a 4px bar`);
    assert.equal(callout.get("border-left-color"), color);
  }
});