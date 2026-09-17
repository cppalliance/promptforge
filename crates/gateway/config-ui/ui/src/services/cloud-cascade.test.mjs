// Pins the cloud cascade derivation: tier grouping with greyed
// providers, kind- and provider-scoped families, canonical rows with
// their variant counts, and the name display rule.
import assert from "node:assert/strict";
import test from "node:test";

import { cloudSheetFixture, loadApp } from "../harness.mjs";

const app = await loadApp();

test("providers group by tier in order, alphabetical within, empty tiers omitted", () => {
  const groups = app.providersByTier(cloudSheetFixture(), "chat");
  assert.deepEqual(
    groups.map((group) => group.tier),
    ["prime", "subprime", "niche"],
    "the sheet has no aggregator tier, so none renders",
  );
  assert.deepEqual(
    groups[0].providers.map((provider) => provider.name),
    ["anthropic", "openai"],
    "alphabetical by display name within the tier",
  );
  assert.deepEqual(
    groups[1].providers.map((provider) => provider.name),
    ["acme"],
  );
  assert.deepEqual(
    groups[2].providers.map((provider) => provider.name),
    ["deepgram"],
  );
});

test("providers with no models of the selected kind are flagged disabled", () => {
  const sheet = cloudSheetFixture();
  const chat = app.providersByTier(sheet, "chat");
  const deepgram = chat.flatMap((group) => group.providers).find((p) => p.name === "deepgram");
  assert.equal(deepgram.disabled, true, "Deepgram serves no chat models");
  const anthropic = chat.flatMap((group) => group.providers).find((p) => p.name === "anthropic");
  assert.equal(anthropic.disabled, false);

  const stt = app.providersByTier(sheet, "transcription");
  const sttDeepgram = stt.flatMap((group) => group.providers).find((p) => p.name === "deepgram");
  assert.equal(sttDeepgram.disabled, false, "Deepgram serves transcription");
  const sttAnthropic = stt.flatMap((group) => group.providers).find((p) => p.name === "anthropic");
  assert.equal(sttAnthropic.disabled, true, "Anthropic serves no transcription");
});

test("families are distinct and scoped to the selected kind and provider", () => {
  const sheet = cloudSheetFixture();
  assert.deepEqual(app.familiesFor(sheet.providers.anthropic, "chat"), [
    "claude-fable",
    "claude-opus",
  ]);
  assert.deepEqual(
    app.familiesFor(sheet.providers.anthropic, "transcription"),
    [],
    "no transcription families on a chat-only provider",
  );
  assert.deepEqual(app.familiesFor(sheet.providers.deepgram, "transcription"), ["nova"]);
});

test("canonical rows carry their variant counts; the family filter narrows them", () => {
  const sheet = cloudSheetFixture();
  const rows = app.canonicalRows(sheet.providers.anthropic, "chat", null);
  assert.deepEqual(
    rows.map((row) => row.entry.id),
    ["claude-fable-5-1", "claude-opus-5"],
    "variants never appear as rows",
  );
  assert.equal(rows[0].variants.length, 2, "both dated snapshots count");
  assert.deepEqual(
    rows[0].variants.map((variant) => variant.variant),
    ["2026-09-01", "2026-07-15"],
  );
  assert.equal(rows[1].variants.length, 0);

  const fableOnly = app.canonicalRows(sheet.providers.anthropic, "chat", "claude-fable");
  assert.deepEqual(
    fableOnly.map((row) => row.entry.id),
    ["claude-fable-5-1"],
  );
});

test("the display rule shows the id beneath a differing display name", () => {
  const sheet = cloudSheetFixture();
  const fable = sheet.providers.anthropic.models[0];
  assert.deepEqual(app.displayRule(fable), {
    primary: "Claude Fable 5.1",
    secondary: "claude-fable-5-1",
  });
  const acme = sheet.providers.acme.models[0];
  assert.deepEqual(
    app.displayRule(acme),
    { primary: "acme-1", secondary: null },
    "an id-equal display name renders once",
  );
});
