// Pins the detail pane: the registry drives the local and remote
// forms, the flash-attention dependency gates cache_type_v, effort
// levels feed the default-effort options, edits raise dirty dots with
// per-field reset, Save PUTs the exact payload with untouched secrets
// still redacted, the pending chip appears once a shadow differs from
// the running view, provenance names the inherited file, and the
// first inherited edit raises the one-time override note.
import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, modelsFixture, navigate, settle } from "../harness.mjs";

function fixtureStub(extra = {}) {
  return gatewayStub({ key: "k", config: modelsFixture(), models: ["qwen-common"], ...extra });
}

/** Commits a text edit the way a user does: type, then change. */
function typeInto(dom, input, value) {
  input.value = value;
  input.dispatchEvent(new dom.window.Event("change"));
}

/** Opens one disclosure and returns its option values. */
function dropdownValues(root, key) {
  const row = root.querySelector(`.field-row[data-key='${key}']`);
  row.querySelector(".select").click();
  return [...row.querySelectorAll(".menu-item")].map((option) => option.dataset.value);
}

test("the local detail pane renders the registry sections with the model's values", async () => {
  const stub = fixtureStub({
    modelInfo: { architecture: "qwen3", layer_count: 32, parameter_count: 8_000_000_000 },
  });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/local/qwen-common");
  await settle();

  for (const id of ["gpu", "generation", "source", "capabilities"]) {
    assert.ok(
      root.querySelector(`.detail-section[data-section='${id}']`),
      `the ${id} section renders`,
    );
  }
  assert.ok(
    root.querySelector(".detail-section[data-section='speculative'] .section-add"),
    "an unconfigured speculative section offers its Add button",
  );

  const gpuRow = root.querySelector(".field-row[data-key='gpu_layers']");
  assert.equal(gpuRow.querySelector(".input-readout").value, "99");
  assert.equal(
    gpuRow.querySelector(".readout-suffix").textContent,
    "/ 32",
    "the readout carries the GGUF layer total from model-info",
  );

  const flash = root.querySelector(".field-row[data-key='flash_attention'] .switch");
  assert.equal(flash.getAttribute("aria-checked"), "true");
  const cacheV = root.querySelector(".field-row[data-key='cache_type_v'] .select");
  assert.equal(cacheV.disabled, false, "cache_type_v is editable while flash attention is on");
  assert.ok(
    root.querySelector(".field-row[data-key='vram_gb']"),
    "vram_gb shows because a local dominion is bound",
  );
  assert.ok(
    root.querySelector(".field-row[data-key='adaptive_thinking']"),
    "adaptive_thinking shows because thinking is not never",
  );

  const kindSelect = root.querySelector(".field-row[data-key='kind'] .select");
  assert.deepEqual(
    dropdownValues(root, "kind"),
    ["chat", "embedding", "classifier"],
    "the header kind dropdown offers the three model kinds",
  );
  assert.equal(kindSelect.value, "chat");

  // Typing the detent name maps to the sentinel and reads back as Max.
  const readout = gpuRow.querySelector(".input-readout");
  typeInto(dom, readout, "Max");
  await settle();
  assert.equal(
    root.querySelector(".field-row[data-key='gpu_layers'] .input-readout").value,
    "Max",
    "the Max detent maps to the sentinel value",
  );
});

test("the chat-template dropdown lists the Rust catalog and writes a built-in family", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/local/qwen-common");
  await settle();

  assert.deepEqual(
    dropdownValues(root, "chat_template_file"),
    [
      "",
      "builtin:chatml",
      "builtin:llama-3",
      "builtin:llama-3.1",
      "builtin:qwen-2.5",
      "builtin:qwen-3",
      "builtin:gemma-3",
      "builtin:gemma-4",
      "builtin:mistral",
      "builtin:phi-3",
      "builtin:phi-4",
      "builtin:gpt-oss",
      "builtin:zephyr",
      "__custom_path__",
    ],
  );
  root
    .querySelector(".field-row[data-key='chat_template_file'] [data-value='builtin:qwen-3']")
    .click();
  await settle();
  root.querySelector(".detail-save").click();
  await settle();

  const put = stub.calls.find(
    (call) => call.url.endsWith("/admin/config") && call.init.method === "PUT",
  );
  const model = JSON.parse(put.init.body).local_model.find(
    (entry) => entry.name === "qwen-common",
  );
  assert.equal(model.chat_template_file, "builtin:qwen-3");
});

test("Custom path reveals a labeled text field and writes its path", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/local/llama-leaf");
  await settle();

  dropdownValues(root, "chat_template_file");
  root
    .querySelector(".field-row[data-key='chat_template_file'] [data-value='__custom_path__']")
    .click();
  const custom = root.querySelector(".chat-template-custom");
  assert.equal(custom.hidden, false);
  assert.equal(custom.querySelector("label").htmlFor, "field-chat_template_file-custom");
  typeInto(dom, custom.querySelector("input"), "templates/llama.jinja");
  await settle();
  root.querySelector(".detail-save").click();
  await settle();

  const put = stub.calls.find(
    (call) => call.url.endsWith("/admin/config") && call.init.method === "PUT",
  );
  const model = JSON.parse(put.init.body).local_model.find(
    (entry) => entry.name === "llama-leaf",
  );
  assert.equal(model.chat_template_file, "templates/llama.jinja");
});

test("a headless gateway without the catalog route degrades to Auto and Custom path", async () => {
  const stub = fixtureStub({ chatTemplates: null });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/local/qwen-common");
  await settle();

  assert.deepEqual(
    dropdownValues(root, "chat_template_file"),
    ["", "__custom_path__"],
    "a 404 catalog leaves the automatic and custom choices",
  );
  assert.match(
    root.querySelector(".chat-template-resolution").textContent,
    /Effective sourceAuto/,
  );
});

test("effective template source, detected family, and known-broken reason render", async () => {
  const catalog = {
    families: [
      { slug: "qwen-3", label: "Qwen 3" },
      { slug: "gemma-4", label: "Gemma 4" },
    ],
    mappings: [],
    models: [
      {
        name: "qwen-common",
        effective_source: "known-override",
        effective_family: "gemma-4",
        detected_family: "qwen-3",
        reason: "Known-broken embedded template matched by content hash.",
      },
    ],
  };
  const stub = fixtureStub({ chatTemplates: catalog });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/local/qwen-common");
  await settle();

  const details = root.querySelector(".chat-template-resolution");
  assert.match(details.textContent, /Effective sourceKnown override - Gemma 4/);
  assert.match(details.textContent, /Detected familyQwen 3/);
  assert.match(details.textContent, /Known-broken embedded template matched by content hash/);
});

test("Auto clears an explicit path and shows automatic resolution", async () => {
  const config = modelsFixture();
  config.local_model[1].chat_template_file = "templates/llama.jinja";
  const catalog = {
    families: [{ slug: "llama-3", label: "Llama 3" }],
    mappings: [],
    models: [
      {
        name: "llama-leaf",
        effective_source: "custom",
        effective_family: null,
        detected_family: "llama-3",
        reason: "Custom template path `templates/llama.jinja` is selected.",
      },
    ],
  };
  const stub = fixtureStub({
    config,
    pending: structuredClone(config),
    chatTemplates: catalog,
  });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/local/llama-leaf");
  await settle();

  dropdownValues(root, "chat_template_file");
  root
    .querySelector(".field-row[data-key='chat_template_file'] [data-value='']")
    .click();
  await settle();
  assert.match(
    root.querySelector(".chat-template-resolution").textContent,
    /Effective sourceAuto/,
  );
  root.querySelector(".detail-save").click();
  await settle();

  const put = stub.calls.find(
    (call) => call.url.endsWith("/admin/config") && call.init.method === "PUT",
  );
  const model = JSON.parse(put.init.body).local_model.find(
    (entry) => entry.name === "llama-leaf",
  );
  assert.equal(model.chat_template_file, null);
});

test("the remote detail pane renders routing fields and effort levels feed default effort", async () => {
  const config = modelsFixture();
  config.model[0].images = true;
  const stub = fixtureStub({ config });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/remote/gpt-remote");
  await settle();

  assert.equal(root.querySelector(".field-row[data-key='upstream'] input").value, "gpt-4.1");
  assert.deepEqual(
    [...root.querySelectorAll(".detail-header .capability-badge")].map(
      (badge) => badge.textContent,
    ),
    ["images", "thinking"],
    "the detail header repeats the model capabilities",
  );
  assert.match(
    root.querySelector(".field-row[data-key='endpoints'] .pill").textContent,
    /openai/,
    "the endpoints multi-select shows the configured endpoint chip",
  );

  const effortSelect = root.querySelector(".field-row[data-key='default_effort'] .select");
  assert.deepEqual(
    dropdownValues(root, "default_effort"),
    ["", "low", "high"],
    "default_effort offers exactly the configured effort levels",
  );
  assert.equal(effortSelect.value, "low");

  const chips = root.querySelector(".field-row[data-key='effort_levels'] .chip-input input");
  chips.value = "medium";
  chips.dispatchEvent(new dom.window.KeyboardEvent("keydown", { key: "Enter" }));
  await settle();
  assert.deepEqual(
    [
      ...root.querySelectorAll(".field-row[data-key='default_effort'] .menu-item"),
    ].map((option) => option.dataset.value),
    ["", "low", "high", "medium"],
    "a new effort level immediately feeds the default-effort options",
  );
});

test("cache_type_v is disabled until flash attention turns on", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/local/llama-leaf");
  await settle();

  assert.equal(
    root.querySelector(".field-row[data-key='cache_type_v'] .select").disabled,
    true,
    "flash attention is off, so the V cache dropdown is disabled",
  );

  root.querySelector(".field-row[data-key='flash_attention'] .switch").click();
  await settle();
  assert.equal(
    root.querySelector(".field-row[data-key='cache_type_v'] .select").disabled,
    false,
    "turning flash attention on enables the V cache dropdown",
  );
});

test("the custom dropdown follows listbox arrow and Enter keyboard behavior", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/remote/gpt-remote");
  await settle();

  const trigger = root.querySelector(".field-row[data-key='default_effort'] .select");
  trigger.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
  );
  assert.equal(trigger.getAttribute("aria-expanded"), "true", "ArrowDown opens the listbox");
  const listbox = root.querySelector(
    ".field-row[data-key='default_effort'] [role='listbox']",
  );
  assert.ok(listbox, "the disclosure owns a listbox");
  assert.ok(listbox.querySelector("[role='option']"), "its rows expose option semantics");
  assert.equal(
    dom.window.document.activeElement?.dataset.value,
    "low",
    "opening focuses the selected option",
  );
  dom.window.document.activeElement.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
  );
  assert.equal(dom.window.document.activeElement?.dataset.value, "high", "ArrowDown moves focus");
  dom.window.document.activeElement.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
  );
  await settle();
  assert.equal(
    root.querySelector(".field-row[data-key='default_effort'] .select").value,
    "high",
    "Enter commits the focused option",
  );
});

test("an edit raises the dirty dot and reset, and reset reverts the field", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/local/llama-leaf");
  await settle();

  assert.equal(root.querySelector(".detail-save").disabled, true, "Save starts disabled");

  typeInto(dom, root.querySelector(".field-row[data-key='description'] textarea"), "tuned");
  await settle();

  const row = root.querySelector(".field-row[data-key='description']");
  assert.ok(row.querySelector(".dirty-dot"), "the edited field shows the dirty dot");
  const reset = row.querySelector(".field-reset");
  assert.equal(reset.getAttribute("aria-label"), "Reset Description");
  assert.equal(root.querySelector(".detail-save").disabled, false, "Save enables when dirty");

  reset.click();
  await settle();
  const reverted = root.querySelector(".field-row[data-key='description']");
  assert.equal(reverted.querySelector("textarea").value, "defined in the leaf");
  assert.equal(reverted.querySelector(".dirty-dot"), null, "reset clears the dot");
  assert.equal(root.querySelector(".detail-save").disabled, true, "Save disables again");
});

test("Save PUTs the edited payload with untouched secrets redacted, then the pending chip appears", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/local/llama-leaf");
  await settle();

  typeInto(dom, root.querySelector(".field-row[data-key='description'] textarea"), "tuned");
  await settle();
  root.querySelector(".detail-save").click();
  await settle();

  const put = stub.calls.find(
    (call) => call.url.endsWith("/admin/config") && call.init.method === "PUT",
  );
  assert.ok(put, "Save PUTs /admin/config");
  const body = JSON.parse(put.init.body);
  const llama = body.local_model.find((entry) => entry.name === "llama-leaf");
  assert.equal(llama.description, "tuned", "the edited field carries its new value");
  assert.equal(
    body.endpoint[0].api_key,
    "***",
    "an untouched secret rides back as the redaction, never a real value",
  );
  assert.equal(body.source_files, undefined, "provenance is stripped from the payload");
  assert.equal(llama.source_file, undefined, "entry provenance is stripped too");

  const row = root.querySelector(".field-row[data-key='description']");
  assert.ok(row.querySelector(".pending-chip"), "the saved field shows the pending chip");
  assert.equal(row.querySelector(".dirty-dot"), null, "saving clears the dirty dot");
  assert.match(
    root.querySelector(".apply-button")?.textContent ?? "",
    /Apply \(1\)/,
    "the tab bar's Apply count follows the dirty report",
  );
});

test("deleting a model confirms, PUTs the config without it, and returns to the list", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/local/llama-leaf");
  await settle();

  root.querySelector(".detail-delete").click();
  await settle();
  const dialog = dom.window.document.querySelector(".confirm-overlay");
  assert.ok(dialog, "delete opens a confirm dialog");
  assert.match(dialog.textContent, /llama-leaf/, "the dialog names the model");
  dialog.querySelector(".button-danger").click();
  await settle();

  const put = stub.calls.find(
    (call) => call.url.endsWith("/admin/config") && call.init.method === "PUT",
  );
  assert.ok(put, "the confirmed delete PUTs the config shadow");
  const names = JSON.parse(put.init.body).local_model.map((entry) => entry.name);
  assert.deepEqual(names, ["qwen-common"], "the payload drops exactly the deleted entry");
  assert.equal(dom.window.location.hash, "#/local", "the route returns to Local");
});

test("the context slider maps log positions to token counts at both ends", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/local/llama-leaf");
  await settle();

  const range = root.querySelector(".field-row[data-key='context'] input[type='range']");
  const initialProgress = Number(range.style.getPropertyValue("--slider-progress"));
  assert.ok(
    initialProgress > 0.32 && initialProgress < 0.35,
    "4096 maps one third across the logarithmic 512-262144 range",
  );
  range.value = range.max;
  range.dispatchEvent(new dom.window.Event("change"));
  await settle();
  assert.equal(
    root.querySelector(".field-row[data-key='context'] .input-readout").value,
    "262144",
    "the top log position maps to the range maximum",
  );

  const top = root.querySelector(".field-row[data-key='context'] input[type='range']");
  top.value = "0";
  top.dispatchEvent(new dom.window.Event("change"));
  await settle();
  assert.equal(
    root.querySelector(".field-row[data-key='context'] .input-readout").value,
    "512",
    "the bottom log position maps to the range minimum",
  );
});

test("the reveal button posts the model's source path", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/local/qwen-common");
  await settle();

  root.querySelector(".reveal-button").click();
  await settle();
  const call = stub.calls.find((c) => c.url.endsWith("/admin/reveal"));
  assert.equal(call?.init.method, "POST", "reveal POSTs /admin/reveal");
  assert.deepEqual(
    JSON.parse(call.init.body),
    { path: "models/Qwen3-8B-Q4_K_M.gguf" },
    "the body names the source path",
  );
});

test("adding a multimodal projector implies the images toggle on and locks it", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/local/llama-leaf");
  await settle();

  root.querySelector(".detail-section[data-section='projector'] .section-add").click();
  await settle();
  const images = root.querySelector(".field-row[data-key='images'] .switch");
  assert.equal(images.getAttribute("aria-checked"), "true", "the toggle shows on");
  assert.equal(images.disabled, true, "the implied toggle is locked");
  assert.ok(
    [...root.querySelectorAll(".detail-header .capability-badge")].some(
      (badge) => badge.textContent === "images",
    ),
    "the implied capability also appears in the detail header",
  );
});
