// Pins the ui layering checker: the current tree passes, and the
// checker itself fails a synthetic violating tree - so a green
// check-layers run means the rule is enforced, not vacuous.
import assert from "node:assert/strict";
import test from "node:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

import { checkImport, runWalk } from "../check-layers.mjs";

test("the current src tree has no layer violations", () => {
  assert.deepEqual(runWalk(), [], "services/components/views obey the layer rule");
});

test("the checker fails a synthetic violating tree", (t) => {
  const src = mkdtempSync(path.join(tmpdir(), "check-layers-"));
  t.after(() => rmSync(src, { recursive: true, force: true }));
  mkdirSync(path.join(src, "services"));
  mkdirSync(path.join(src, "components"));
  mkdirSync(path.join(src, "views"));
  writeFileSync(path.join(src, "views", "thing.ts"), "export const thing = 1;\n");
  writeFileSync(
    path.join(src, "components", "widget.ts"),
    'import { thing } from "../views/thing";\nexport const widget = thing;\n',
  );
  writeFileSync(
    path.join(src, "services", "bad.ts"),
    'import { thing } from "../views/thing";\n' +
      'import { widget } from "../components/widget";\n' +
      "export const bad = thing + widget;\n",
  );

  const violations = runWalk(src);
  assert.equal(violations.length, 3, `every synthetic violation is caught: ${violations}`);
  assert.ok(
    violations.some((v) => v.includes("services/bad.ts") && v.includes("views/thing")),
    "services importing a view is a violation",
  );
  assert.ok(
    violations.some((v) => v.includes("services/bad.ts") && v.includes("components/widget")),
    "services importing a component is a violation",
  );
  assert.ok(
    violations.some((v) => v.includes("components/widget.ts") && v.includes("views/thing")),
    "a component importing a view is a violation",
  );
});

test("allowed directions pass and the composition root is import-proof", () => {
  const src = path.join(tmpdir(), "check-layers-static", "src");
  const at = (...parts) => path.join(src, ...parts);
  assert.equal(checkImport(at("views", "v.ts"), at("services", "s"), src), null);
  assert.equal(checkImport(at("views", "v.ts"), at("components", "c"), src), null);
  assert.equal(checkImport(at("components", "c.ts"), at("services", "s"), src), null);
  assert.match(
    checkImport(at("views", "v.ts"), at("main.ts"), src),
    /composition root/,
    "nothing may import main.ts",
  );
});
