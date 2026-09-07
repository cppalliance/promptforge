import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("the config store contains no legacy STT canonicalization", async () => {
  const source = await readFile(new URL("./config-store.ts", import.meta.url), "utf8");

  assert.doesNotMatch(source, /\bcanonicalizeStt\b/);
  assert.doesNotMatch(source, /workshop\["stt"\]/);
});
