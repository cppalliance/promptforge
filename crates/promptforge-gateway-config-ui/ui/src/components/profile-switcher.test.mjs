// Pins the profile switcher: the menu lists every profile with the
// active one checked, selecting another posts the switch and shows the
// stage overlay, rows disable while the switch is in flight, and the
// terminal ready event closes the overlay and updates the trigger.
import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, settle, sseChannel } from "../harness.mjs";

test("the menu lists every profile with the active one checked", async () => {
  const stub = gatewayStub({ profile: "default", profiles: ["default", "fast"] });
  const { root } = await bootApp({ key: "k", stub });

  root.querySelector(".profile-switcher button").click();
  await settle();

  const rows = [...root.querySelectorAll("[role='menuitemradio']")];
  assert.equal(rows.length, 2, "both profiles are listed");
  assert.deepEqual(
    rows.map((row) => [row.textContent, row.getAttribute("aria-checked")]),
    [
      ["default", "true"],
      ["fast", "false"],
    ],
    "only the active profile is checked",
  );
});

test("selecting a profile posts the switch, shows the overlay, and disables rows", async () => {
  const channel = sseChannel();
  const stub = gatewayStub({
    profile: "default",
    profiles: ["default", "fast"],
    onSwitch: () => channel.response,
  });
  const { dom, root } = await bootApp({ key: "k", stub });

  root.querySelector(".profile-switcher button").click();
  await settle();
  const rows = [...root.querySelectorAll("[role='menuitemradio']")];
  rows.find((row) => row.textContent === "fast").click();
  await settle();

  const switchCall = stub.calls.find((call) => call.url.endsWith("/admin/switch-profile"));
  assert.ok(switchCall, "the switch was posted");
  assert.equal(switchCall.init.method, "POST");
  assert.deepEqual(JSON.parse(switchCall.init.body), { name: "fast" });

  const doc = dom.window.document;
  assert.ok(doc.querySelector(".apply-overlay"), "the stage overlay is up");
  assert.ok(
    rows.every((row) => row.disabled),
    "every row is disabled while the switch is in flight",
  );

  channel.push({ stage: "loading-profile" });
  await settle();
  const stage = doc.querySelector(".apply-overlay [data-stage='loading-profile']");
  assert.ok(stage.classList.contains("is-active"), "the streamed stage shows as active");

  channel.push({ stage: "starting-models" });
  await settle();
  assert.ok(
    doc
      .querySelector(".apply-overlay [data-stage='loading-profile']")
      .classList.contains("is-done"),
    "an earlier stage completes when the next one begins",
  );

  channel.push({ status: "ready", profile: "fast" });
  channel.end();
  await settle();

  assert.equal(doc.querySelector(".apply-overlay"), null, "the terminal event closes the overlay");
  const trigger = root.querySelector(".profile-switcher button");
  assert.match(trigger.textContent, /fast/, "the trigger shows the new active profile");
  assert.ok(root.querySelector(".profile-switcher .menu").hidden, "the menu closed");
});

test("opening the menu moves focus to the active row and Escape returns it", async () => {
  const stub = gatewayStub({ profile: "default", profiles: ["default", "fast"] });
  const { dom, root } = await bootApp({ key: "k", stub });

  const trigger = root.querySelector(".profile-switcher button");
  trigger.click();
  await settle();

  const doc = dom.window.document;
  const activeRow = root.querySelector("[aria-checked='true']");
  assert.equal(doc.activeElement, activeRow, "focus lands on the active row");

  activeRow.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
  );
  assert.ok(root.querySelector(".profile-switcher .menu").hidden, "Escape closes the menu");
  assert.equal(doc.activeElement, trigger, "Escape returns focus to the trigger");
});

test("the overlay takes focus while a switch is in flight", async () => {
  const channel = sseChannel();
  const stub = gatewayStub({
    profile: "default",
    profiles: ["default", "fast"],
    onSwitch: () => channel.response,
  });
  const { dom, root } = await bootApp({ key: "k", stub });

  root.querySelector(".profile-switcher button").click();
  await settle();
  [...root.querySelectorAll("[role='menuitemradio']")]
    .find((row) => row.textContent === "fast")
    .click();
  await settle();

  const doc = dom.window.document;
  const card = doc.querySelector(".apply-overlay .modal");
  assert.equal(card.getAttribute("role"), "alertdialog");
  assert.equal(card.getAttribute("aria-modal"), "true");
  assert.equal(doc.activeElement, card, "focus moved into the dialog");

  channel.push({ status: "ready", profile: "fast" });
  channel.end();
  await settle();
  assert.equal(
    doc.activeElement,
    root.querySelector(".profile-switcher button"),
    "focus returns to the trigger after the switch",
  );
});

test("an SSE event split across chunk boundaries still parses", async () => {
  const channel = sseChannel();
  const stub = gatewayStub({
    profile: "default",
    profiles: ["default", "fast"],
    onSwitch: () => channel.response,
  });
  const { dom, root } = await bootApp({ key: "k", stub });

  root.querySelector(".profile-switcher button").click();
  await settle();
  [...root.querySelectorAll("[role='menuitemradio']")]
    .find((row) => row.textContent === "fast")
    .click();
  await settle();

  channel.pushRaw('data: {"stage": "load');
  await settle();
  channel.pushRaw('ing-profile"}\n\n');
  await settle();

  const doc = dom.window.document;
  const stage = doc.querySelector(".apply-overlay [data-stage='loading-profile']");
  assert.ok(
    stage?.classList.contains("is-active"),
    "the split event reassembled into the stage marker",
  );

  channel.push({ status: "ready", profile: "fast" });
  channel.end();
  await settle();
  assert.equal(doc.querySelector(".apply-overlay"), null, "the switch still completes");
});

test("an unknown stage from the gateway is appended and rendered", async () => {
  const channel = sseChannel();
  const stub = gatewayStub({
    profile: "default",
    profiles: ["default", "fast"],
    onSwitch: () => channel.response,
  });
  const { dom, root } = await bootApp({ key: "k", stub });

  root.querySelector(".profile-switcher button").click();
  await settle();
  [...root.querySelectorAll("[role='menuitemradio']")]
    .find((row) => row.textContent === "fast")
    .click();
  await settle();

  channel.push({ stage: "warming-cache" });
  await settle();

  const doc = dom.window.document;
  const row = doc.querySelector(".apply-overlay [data-stage='warming-cache']");
  assert.ok(row, "the unknown stage gets its own row");
  assert.ok(row.classList.contains("is-active"), "the appended row shows as active");
  assert.match(row.textContent, /warming-cache/, "the raw stage id is the label");

  channel.push({ status: "ready", profile: "fast" });
  channel.end();
  await settle();
});

test("a stream that drops without a terminal event surfaces the error", async () => {
  const channel = sseChannel();
  const stub = gatewayStub({
    profile: "default",
    profiles: ["default", "fast"],
    onSwitch: () => channel.response,
  });
  const { dom, root } = await bootApp({ key: "k", stub });

  root.querySelector(".profile-switcher button").click();
  await settle();
  [...root.querySelectorAll("[role='menuitemradio']")]
    .find((row) => row.textContent === "fast")
    .click();
  await settle();

  channel.push({ stage: "loading-profile" });
  channel.end();
  await settle();

  const doc = dom.window.document;
  const toast = doc.querySelector(".toast-error");
  assert.match(
    toast?.textContent ?? "",
    /without a terminal event/,
    "the dropped connection reaches a toast, not a silent stall",
  );
  const failed = doc.querySelector(".apply-overlay [data-stage='loading-profile']");
  assert.ok(failed?.classList.contains("is-failed"), "the active stage shows the error mark");
  const trigger = root.querySelector(".profile-switcher button");
  assert.match(trigger.textContent, /default/, "the trigger keeps the old active profile");
});

test("a terminal error keeps the old profile and surfaces a toast", async () => {
  const channel = sseChannel();
  const stub = gatewayStub({
    profile: "default",
    profiles: ["default", "fast"],
    onSwitch: () => channel.response,
  });
  const { dom, root } = await bootApp({ key: "k", stub });

  root.querySelector(".profile-switcher button").click();
  await settle();
  [...root.querySelectorAll("[role='menuitemradio']")]
    .find((row) => row.textContent === "fast")
    .click();
  await settle();

  channel.push({ stage: "loading-profile" });
  channel.push({ status: "error", message: "profile not found" });
  channel.end();
  await settle();

  const doc = dom.window.document;
  const toast = doc.querySelector(".toast-error");
  assert.equal(toast?.textContent, "profile not found", "the failure reaches a toast");
  const failed = doc.querySelector(".apply-overlay [data-stage='loading-profile']");
  assert.ok(failed?.classList.contains("is-failed"), "the dying stage shows the error mark");
  const trigger = root.querySelector(".profile-switcher button");
  assert.match(trigger.textContent, /default/, "the trigger keeps the old active profile");
});
