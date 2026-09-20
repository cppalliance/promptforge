// Unit test for the WorkshopPart base class (src/base/workshop-part.ts):
// the Part/Composite hierarchy root every dockview panel extends. Bundles
// the module with esbuild and drives a concrete subclass against jsdom.
// Covers: the element exists before init, init runs create() exactly once
// with the element as the parent (a second init does not rebuild),
// layout() accepts a dimension, and dispose() releases children registered
// through the inherited _register.
// Run: node test/workshop-part.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.HTMLElement = dom.window.HTMLElement;

const bundle = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "src", "base", "workshop-part.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});
const { WorkshopPart } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

class TestPart extends WorkshopPart {
  createCount = 0;
  create(parent) {
    this.createCount += 1;
    this.createParent = parent;
    const child = document.createElement("span");
    child.className = "test-part-child";
    parent.appendChild(child);
  }
}

const part = new TestPart();
check("the element exists before init", part.element instanceof HTMLElement);
check("create has not run before init", part.createCount === 0);

const params = { params: {}, api: {} };
part.init(params);
check("init runs create once", part.createCount === 1);
check("create receives the part's own element as the parent", part.createParent === part.element);
check("create built the part's content into the element", !!part.element.querySelector(".test-part-child"));
part.init(params);
check("a second init does not rebuild the content", part.createCount === 1);

// layout() takes a dimension and is a no-op by default.
let layoutThrew = false;
try {
  part.layout({ width: 800, height: 600 });
} catch {
  layoutThrew = true;
}
check("layout accepts a dimension", !layoutThrew);

// dispose() releases children registered through the inherited _register.
let childDisposed = false;
part._register({ dispose: () => (childDisposed = true) });
part.dispose();
check("dispose releases registered children", childDisposed);

if (failures.length > 0) {
  console.error(`workshop-part: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("workshop-part: all assertions passed");
process.exit(0);
