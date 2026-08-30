// Pins the Profiles view: the list renders with the active pill, Set
// Active runs the switch flow through the stage overlay, delete
// confirms and the active profile's 409 surfaces, the New Profile
// dialog posts each mode's body and blocks bad names client-side, the
// include chain editor reads its rows from the payload's `include`
// array (payload order, a fully-overridden parent included), flags a
// missing file, and saves an explicit include array, the drill-in
// edits one include file's provenance-derived content through
// PUT /admin/include/{path}, and the drill-in route parses. An
// outside ../ include renders with no Edit and no remove, the kebab
// menu honors the role="menu" keyboard contract, and the New Profile
// dialog traps Tab and restores focus on Escape.
import assert from "node:assert/strict";
import test from "node:test";

import {
  bootApp,
  gatewayStub,
  jsonResponse,
  loadApp,
  modelsFixture,
  navigate,
  settle,
  sseChannel,
} from "../harness.mjs";

/** A stub over the leaf+common fixture with both files listed. */
function fixtureStub(extra = {}) {
  return gatewayStub({
    profile: "default",
    profiles: ["default", "common"],
    config: modelsFixture(),
    ...extra,
  });
}

/** The fixture with an include entry naming a file that is not on disk. */
function ghostFixture() {
  const config = modelsFixture();
  config.include = ["common.toml", "ghost.toml"];
  return config;
}

/** Opens a profile row's kebab menu and returns its items by label. */
function openKebab(root, name) {
  const row = [...root.querySelectorAll(".profile-row")].find(
    (entry) => entry.querySelector(".profile-name").textContent === name,
  );
  assert.ok(row, `a row for ${name} exists`);
  row.querySelector(".kebab-button").click();
  const items = {};
  for (const item of row.querySelectorAll(".menu-item")) {
    items[item.textContent] = item;
  }
  return items;
}

test("the drill-in route parses and a malformed escape does not", async () => {
  const app = await loadApp();
  assert.deepEqual(app.matchRoute("#/profiles"), { view: "profiles" });
  assert.deepEqual(app.matchRoute("#/profiles/include/common.toml"), {
    view: "profiles",
    detail: "common.toml",
  });
  assert.deepEqual(app.matchRoute(`#/profiles/include/${encodeURIComponent("../gateway.toml")}`), {
    view: "profiles",
    detail: "../gateway.toml",
  });
  assert.equal(app.matchRoute("#/profiles/include/%"), null);
  assert.equal(app.matchRoute("#/profiles/other"), null);
});

test("the list renders the active pill and the summary counts for the active profile", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles");
  await settle();

  const rows = [...root.querySelectorAll(".profile-row")];
  assert.equal(rows.length, 2, "both profiles are listed");
  const active = rows.find((row) => row.querySelector(".profile-name").textContent === "default");
  assert.ok(active.querySelector(".active-pill"), "the active row carries the Active pill");
  assert.ok(active.querySelector(".status-dot.is-ok"), "the active row's dot is green");
  const other = rows.find((row) => row.querySelector(".profile-name").textContent === "common");
  assert.equal(other.querySelector(".active-pill"), null, "a non-active row has no pill");

  const summary = root.querySelector(".profile-summary-pane");
  assert.match(summary.querySelector(".profile-counts").textContent, /2 local models/);
  assert.match(summary.querySelector(".profile-counts").textContent, /1 remote model/);
  assert.match(
    summary.querySelector(".profile-allowlist").textContent,
    /All models visible/,
    "no allowlist renders as all-visible",
  );

  other.querySelector(".profile-select").click();
  await settle();
  const inactive = root.querySelector(".profile-summary-pane");
  assert.match(
    inactive.querySelector(".profile-inactive-note").textContent,
    /active profile only/,
    "a non-active profile states counts and chain are active-only",
  );
  assert.equal(inactive.querySelector(".include-chain"), null, "no chain editor off-active");
});

test("Set Active runs the switch flow through the stage overlay", async () => {
  const channel = sseChannel();
  const stub = fixtureStub({ onSwitch: () => channel.response });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles");
  await settle();

  openKebab(root, "common")["Set Active"].click();
  await settle();

  const doc = dom.window.document;
  assert.ok(doc.querySelector(".apply-overlay"), "the stage overlay is up");
  const switchCall = stub.calls.find((call) => call.url.endsWith("/admin/switch-profile"));
  assert.deepEqual(JSON.parse(switchCall.init.body), { name: "common" });

  channel.push({ stage: "loading-profile" });
  await settle();
  assert.ok(
    doc
      .querySelector(".apply-overlay [data-stage='loading-profile']")
      .classList.contains("is-active"),
    "the streamed stage shows as active",
  );

  stub.state.active = "common";
  channel.push({ status: "ready", profile: "common" });
  channel.end();
  await settle();

  assert.equal(doc.querySelector(".apply-overlay"), null, "the terminal event closes the overlay");
  const rows = [...root.querySelectorAll(".profile-row")];
  const common = rows.find((row) => row.querySelector(".profile-name").textContent === "common");
  assert.ok(common.querySelector(".active-pill"), "the pill follows the switch");
});

test("delete confirms and removes; the active profile's 409 surfaces as a toast", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles");
  await settle();

  openKebab(root, "common")["Delete\u2026"].click();
  await settle();
  const doc = dom.window.document;
  assert.ok(doc.querySelector(".confirm-overlay"), "delete asks first");
  doc.querySelector(".confirm-overlay .button-danger").click();
  await settle();

  const deleteCall = stub.calls.find(
    (call) => call.init.method === "DELETE" && call.url.includes("/admin/profiles/"),
  );
  assert.ok(deleteCall.url.endsWith("/admin/profiles/common"), "the DELETE names the profile");
  assert.equal(
    [...root.querySelectorAll(".profile-name")].some((name) => name.textContent === "common"),
    false,
    "the deleted profile leaves the list",
  );

  openKebab(root, "default")["Delete\u2026"].click();
  await settle();
  doc.querySelector(".confirm-overlay .button-danger").click();
  await settle();
  assert.match(
    doc.querySelector(".toast-error").textContent,
    /active/,
    "the server's active-profile refusal surfaces",
  );
  assert.ok(
    [...root.querySelectorAll(".profile-name")].some((name) => name.textContent === "default"),
    "the active profile survives",
  );
});

test("the New Profile dialog posts each mode's body", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles");
  await settle();
  const doc = dom.window.document;

  const create = async (name, setup) => {
    root.querySelector(".new-profile").click();
    const dialog = doc.querySelector(".new-profile-dialog");
    assert.ok(dialog, "the dialog opens");
    dialog.querySelector("#new-profile-name").value = name;
    if (setup) {
      setup(dialog);
    }
    dialog.querySelector("button[type='submit']").click();
    await settle();
  };

  await create("blank");
  await create("copied", (dialog) => {
    const radio = dialog.querySelector("#start-from-copy");
    radio.checked = true;
    radio.dispatchEvent(new dom.window.Event("change", { bubbles: true }));
    dialog.querySelector("#new-profile-copy-from").value = "common";
  });
  await create("leaf", (dialog) => {
    const radio = dialog.querySelector("#start-from-include");
    radio.checked = true;
    radio.dispatchEvent(new dom.window.Event("change", { bubbles: true }));
    dialog.querySelector("#new-profile-include-from").value = "common";
  });

  const bodies = stub.calls
    .filter((call) => call.init.method === "POST" && call.url.includes("/admin/profiles/"))
    .map((call) => [call.url.slice(call.url.lastIndexOf("/") + 1), JSON.parse(call.init.body)]);
  assert.deepEqual(bodies, [
    ["blank", { mode: "empty" }],
    ["copied", { mode: "copy", from: "common" }],
    ["leaf", { mode: "include", from: "common" }],
  ]);
  assert.ok(
    [...root.querySelectorAll(".profile-name")].some((name) => name.textContent === "leaf"),
    "the refreshed list carries the created profile",
  );
});

test("the dialog blocks an invalid name client-side and surfaces a server 409", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles");
  await settle();
  const doc = dom.window.document;

  root.querySelector(".new-profile").click();
  const dialog = doc.querySelector(".new-profile-dialog");
  dialog.querySelector("#new-profile-name").value = "../escape";
  dialog.querySelector("button[type='submit']").click();
  await settle();
  assert.match(
    dialog.querySelector(".dialog-error").textContent,
    /single file name/,
    "a separator-bearing name is refused before any request",
  );
  assert.equal(
    stub.calls.some((call) => call.init.method === "POST" && call.url.includes("/admin/profiles/")),
    false,
    "no create request left the dialog",
  );

  dialog.querySelector("#new-profile-name").value = "common";
  dialog.querySelector("button[type='submit']").click();
  await settle();
  assert.match(
    dialog.querySelector(".dialog-error").textContent,
    /already exists/,
    "the server's 409 sentence lands in the dialog",
  );
  assert.ok(doc.querySelector(".new-profile-dialog"), "the dialog survives the refusal");
});

test("the chain editor lists the payload's include array and flags a missing file", async () => {
  const stub = gatewayStub({
    profile: "default",
    profiles: ["default", "common"],
    config: ghostFixture(),
  });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles");
  await settle();

  const rows = [...root.querySelectorAll(".chain-row")];
  assert.deepEqual(
    rows.map((row) => row.querySelector(".chain-path").textContent),
    ["common.toml", "ghost.toml"],
    "the chain lists the payload's include entries verbatim",
  );
  const ghost = rows[1];
  assert.ok(ghost.classList.contains("is-missing"), "the unlisted file is flagged");
  assert.equal(ghost.querySelector(".chain-missing").textContent, "Missing");
  assert.equal(ghost.querySelector(".chain-edit"), null, "a missing file offers no drill-in");
  assert.equal(
    rows[0].querySelector(".chain-edit").getAttribute("href"),
    "#/profiles/include/common.toml",
    "an existing file's Edit links to the drill-in route",
  );
});

test("chain rows follow the payload's include order, not alphabetical", async () => {
  // The gateway serves the leaf's include line verbatim; re-sorting the
  // rows (the old provenance approximation) would flip zeta/alpha here
  // and mislead the operator about override precedence.
  const config = modelsFixture();
  config.include = ["zeta.toml", "alpha.toml"];
  const stub = gatewayStub({
    profile: "default",
    profiles: ["alpha", "default", "zeta"],
    config,
  });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles");
  await settle();

  const rows = [...root.querySelectorAll(".chain-row")];
  assert.deepEqual(
    rows.map((row) => row.querySelector(".chain-path").textContent),
    ["zeta.toml", "alpha.toml"],
    "rows keep the on-disk include order",
  );
  assert.equal(
    root.querySelector(".chain-derived-note"),
    null,
    "the order is authoritative, so no approximation note renders",
  );
});

test("a fully-overridden parent appears in the chain and survives a save", async () => {
  // `overridden.toml` supplies no winning value, so it has no provenance
  // anywhere in the fixture; membership must come from the include array
  // or the file would be invisible and silently dropped on a chain save.
  const config = modelsFixture();
  config.include = ["overridden.toml", "common.toml"];
  const stub = gatewayStub({
    profile: "default",
    profiles: ["common", "default", "overridden"],
    config,
  });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles");
  await settle();

  const rows = [...root.querySelectorAll(".chain-row")];
  assert.deepEqual(
    rows.map((row) => row.querySelector(".chain-path").textContent),
    ["overridden.toml", "common.toml"],
    "the provenance-free parent renders as a chain row",
  );
  const overridden = rows[0];
  assert.equal(overridden.classList.contains("is-missing"), false, "it exists on disk");
  assert.equal(
    overridden.querySelector(".chain-edit").getAttribute("href"),
    "#/profiles/include/overridden.toml",
    "the row still offers the drill-in link",
  );

  // Reorder and save: the invisible-parent bug dropped such a file from
  // the explicit array the editor writes.
  overridden.querySelector(".chain-down").click();
  await settle();
  root.querySelector(".chain-save").click();
  await settle();
  const save = stub.calls.find(
    (call) => call.init.method === "PUT" && call.url.endsWith("/admin/config"),
  );
  assert.deepEqual(
    JSON.parse(save.init.body).include,
    ["common.toml", "overridden.toml"],
    "the saved array keeps the provenance-free parent",
  );
});

test("a boot-file include renders as an outside ../ row: no Edit, no remove", async () => {
  const config = modelsFixture();
  config.include = ["../gateway.toml", "common.toml"];
  const stub = gatewayStub({
    profile: "default",
    profiles: ["default", "common"],
    config,
  });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles");
  await settle();

  const rows = [...root.querySelectorAll(".chain-row")];
  assert.deepEqual(
    rows.map((row) => row.querySelector(".chain-path").textContent),
    ["../gateway.toml", "common.toml"],
    "the boot file renders by its verbatim ../ include entry, in payload order",
  );
  const boot = rows[0];
  assert.equal(boot.classList.contains("is-missing"), false, "an outside file is never Missing");
  assert.equal(boot.querySelector(".chain-edit"), null, "an outside file offers no drill-in");
  assert.equal(boot.querySelector(".chain-remove"), null, "an outside file offers no remove");
  assert.ok(boot.querySelector(".chain-down"), "an outside file still reorders");
  assert.ok(rows[1].querySelector(".chain-remove"), "a profile-dir file keeps its remove");
});

test("the kebab menu moves focus in on open, arrows walk it, and Escape returns", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles");
  await settle();
  const doc = dom.window.document;

  const row = [...root.querySelectorAll(".profile-row")].find(
    (entry) => entry.querySelector(".profile-name").textContent === "common",
  );
  const trigger = row.querySelector(".kebab-button");
  trigger.click();
  const items = [...row.querySelectorAll(".menu-item")];
  assert.equal(doc.activeElement, items[0], "opening the menu focuses the first enabled item");

  doc.activeElement.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
  );
  assert.equal(doc.activeElement, items[1], "ArrowDown walks to the next item");
  doc.activeElement.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
  );
  assert.equal(doc.activeElement, items[0], "ArrowDown wraps at the end");

  doc.activeElement.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
  );
  assert.equal(doc.activeElement, trigger, "Escape closes and returns focus to the trigger");
  assert.equal(row.querySelector(".kebab-menu").hidden, true, "the menu is closed");
});

test("the New Profile dialog traps Tab and Escape restores the opener's focus", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles");
  await settle();
  const doc = dom.window.document;

  const opener = root.querySelector(".new-profile");
  opener.focus();
  opener.click();
  const dialog = doc.querySelector(".new-profile-dialog");
  const nameInput = dialog.querySelector("#new-profile-name");
  assert.equal(doc.activeElement, nameInput, "the dialog opens with the name input focused");

  const submit = dialog.querySelector("button[type='submit']");
  submit.focus();
  submit.dispatchEvent(new dom.window.KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
  assert.equal(doc.activeElement, nameInput, "Tab past the last control wraps to the first");
  nameInput.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true }),
  );
  assert.equal(doc.activeElement, submit, "Shift+Tab past the first control wraps to the last");

  doc.activeElement.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
  );
  assert.equal(doc.querySelector(".new-profile-dialog"), null, "Escape closes the dialog");
  assert.equal(doc.activeElement, opener, "focus returns to the opener");
});

test("reorder, remove, and add produce the explicit include array in the save payload", async () => {
  const stub = gatewayStub({
    profile: "default",
    profiles: ["default", "common"],
    config: ghostFixture(),
  });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles");
  await settle();

  // [common, ghost] -> down(common) -> [ghost, common] -> remove(ghost)
  // -> [common] -> add extra.toml (created empty first) -> [common, extra].
  root.querySelectorAll(".chain-row")[0].querySelector(".chain-down").click();
  await settle();
  root.querySelectorAll(".chain-row")[0].querySelector(".chain-remove").click();
  await settle();
  const input = root.querySelector(".chain-add-input");
  input.value = "extra.toml";
  root.querySelector(".chain-add-button").click();
  await settle();

  const created = stub.calls.find(
    (call) => call.init.method === "POST" && call.url.endsWith("/admin/profiles/extra"),
  );
  assert.ok(created, "a new include file is created before it enters the chain");
  assert.deepEqual(JSON.parse(created.init.body), { mode: "empty" });

  root.querySelector(".chain-save").click();
  await settle();
  const save = stub.calls.find(
    (call) => call.init.method === "PUT" && call.url.endsWith("/admin/config"),
  );
  const body = JSON.parse(save.init.body);
  assert.deepEqual(
    body.include,
    ["common.toml", "extra.toml"],
    "the payload carries the explicit reordered include array",
  );
  assert.equal(body.source_files, undefined, "provenance never rides back");
  assert.equal(body.server, undefined, "boot-owned sections stay out of the leaf payload");
});

test("a chain-save validation refusal surfaces on the banner", async () => {
  const stub = fixtureStub({
    onPutConfig: () =>
      jsonResponse({ error: { message: "include cycle: common.toml includes itself" } }, 422),
  });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles");
  await settle();

  root.querySelector(".chain-row .chain-remove").click();
  await settle();
  root.querySelector(".chain-save").click();
  await settle();

  assert.match(
    root.querySelector(".include-chain .banner-danger").textContent,
    /include cycle/,
    "the server's cycle refusal lands on the chain banner",
  );
});

test("the drill-in renders the file's provenance-derived content and saves its shadow", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles/include/common.toml");
  await settle();

  const crumbs = root.querySelector(".breadcrumbs");
  assert.equal(crumbs.querySelector("a.crumb").getAttribute("href"), "#/profiles");
  assert.equal(crumbs.querySelector("a.crumb").textContent, "default.toml");
  assert.equal(crumbs.querySelector(".crumb-current").textContent, "common.toml");

  const keys = [...root.querySelectorAll(".drill-field")].map((field) => field.dataset.key);
  assert.deepEqual(keys, ["dominion", "local_model"], "only common-attributed sections render");
  const models = root.querySelector("#include-local_model");
  assert.match(models.value, /qwen-common/, "the inherited entry belongs to the file");
  assert.doesNotMatch(models.value, /llama-leaf/, "a leaf-owned entry never leaks in");

  const dominion = root.querySelector("#include-dominion");
  dominion.value = dominion.value.replace('"max_queue": 100', '"max_queue": 50');
  dominion.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
  root.querySelector(".drill-save").click();
  await settle();

  const saved = stub.state.includes["common.toml"];
  assert.ok(saved, "the save PUT the include path's shadow");
  assert.equal(saved.dominion[0].max_queue, 50, "the edited value rides in the body");
  assert.equal(saved.local_model[0].name, "qwen-common", "untouched sections ride along");

  navigate(dom, "#/profiles");
  await settle();
  assert.ok(root.querySelector(".profile-list"), "the breadcrumb target renders the list");
});

test("a drill-in JSON parse failure blocks the save with the section named", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/profiles/include/common.toml");
  await settle();

  const dominion = root.querySelector("#include-dominion");
  dominion.value = "{ not json";
  dominion.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
  root.querySelector(".drill-save").click();
  await settle();

  assert.match(
    root.querySelector(".banner-danger").textContent,
    /dominion section is not valid JSON/,
    "the refusal names the broken section",
  );
  assert.equal(stub.state.includes["common.toml"], undefined, "nothing was saved");
});
