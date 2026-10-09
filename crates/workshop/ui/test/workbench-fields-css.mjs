// Source-text contract for the workbench input and select values in the
// component token sheet (src/tokens/component.css), which the run panel's
// selects and fields take: 26px tall, a 2px radius, a 1px #F0F0F013 border;
// selects padded 2px 23px 2px 8px with a 16px caret 6px from the right.
// jsdom applies no layout, so the values are read from the sheets and
// followed through the token tiers (test/helpers/css-values.mjs) down to the
// literals Cursor's workbench uses.
// Run: node test/workbench-fields-css.mjs
import { resolver } from "./helpers/css-values.mjs";

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const resolve = await resolver();
const token = (name) => resolve(`var(${name})`);

check("a workbench field is 26px tall", token("--ws-workbench-input-height") === "26px");
check("a workbench field has a 2px radius", token("--ws-workbench-input-radius") === "2px");
check("a workbench field has a 1px #F0F0F013 border", token("--ws-workbench-input-border") === "1px solid #f0f0f013");
check("a workbench select pads 2px 23px 2px 8px", token("--ws-workbench-select-padding") === "2px 23px 2px 8px");
check("a workbench select's caret is 16px", token("--ws-workbench-select-caret-size") === "16px");
check("a workbench select's caret sits 6px from the right", token("--ws-workbench-select-caret-inset") === "6px");

if (failures.length > 0) {
  console.error(`workbench-fields-css: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("workbench-fields-css: all assertions passed");
