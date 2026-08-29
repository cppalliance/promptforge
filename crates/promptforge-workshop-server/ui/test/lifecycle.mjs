// Unit test for the lifecycle primitives (src/base/lifecycle.ts) and the
// event emitter (src/base/event.ts). Bundles each TS module with esbuild
// and imports it via a data URL. Covers: _register ties children to the
// parent and dispose cascades down the tree; DisposableStore disposes all
// held items in insertion order, tolerates double-dispose, and disposes
// late additions immediately;
// Emitter delivers to subscribers, the returned disposable unsubscribes,
// and nothing is delivered after the emitter is disposed.
// Run: node test/lifecycle.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

async function loadModule(relative) {
  const bundle = await esbuild.build({
    entryPoints: [path.join(uiDir, "..", "src", relative)],
    bundle: true,
    write: false,
    format: "esm",
    platform: "browser",
    target: "es2022",
    logLevel: "silent",
  });
  const code = bundle.outputFiles[0].text;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

const { Disposable, DisposableStore } = await loadModule(
  path.join("base", "lifecycle.ts"),
);
const { Emitter } = await loadModule(path.join("base", "event.ts"));

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// A leaf disposable that records how many times it was disposed.
function leaf(onDispose) {
  const item = {
    disposeCount: 0,
    dispose: () => {
      item.disposeCount++;
      onDispose?.();
    },
  };
  return item;
}

// --- DisposableStore: dispose-all, double-dispose, late add ------------------

{
  const store = new DisposableStore();
  const order = [];
  const first = store.add(leaf(() => order.push("first")));
  const second = store.add(leaf(() => order.push("second")));
  check("add returns the item it was given", store.add(leaf()).disposeCount === 0);
  store.dispose();
  check("dispose releases every held item", first.disposeCount === 1 && second.disposeCount === 1);
  check("items are disposed in insertion order", order.join(",") === "first,second");
  store.dispose();
  check("double-dispose does not re-dispose items", first.disposeCount === 1);
  const late = store.add(leaf());
  check("adding to a disposed store disposes immediately", late.disposeCount === 1);
}

// --- Disposable base: _register routing and cascade --------------------------

{
  class Child extends Disposable {
    constructor() {
      super();
      this.item = this._register(leaf());
    }
  }
  class Parent extends Disposable {
    constructor() {
      super();
      this.child = this._register(new Child());
      this.item = this._register(leaf());
    }
  }
  const parent = new Parent();
  check(
    "_register returns the child it was given",
    parent.item.disposeCount === 0 && parent.child instanceof Child,
  );
  parent.dispose();
  check("dispose releases directly registered items", parent.item.disposeCount === 1);
  check("dispose cascades to registered children", parent.child.item.disposeCount === 1);
  parent.dispose();
  check("double-dispose of a Disposable is harmless", parent.item.disposeCount === 1);
}

// --- Emitter: subscribe, fire, unsubscribe, dispose ---------------------------

{
  const emitter = new Emitter();
  const seen = [];
  const subscription = emitter.event((value) => seen.push(value));
  emitter.fire("one");
  check("a subscriber receives fired values", seen.join(",") === "one");
  subscription.dispose();
  emitter.fire("two");
  check("an unsubscribed listener receives nothing", seen.join(",") === "one");

  const kept = [];
  emitter.event((value) => kept.push(value));
  emitter.fire("three");
  emitter.dispose();
  emitter.fire("four");
  check("nothing is delivered after the emitter is disposed", kept.join(",") === "three");
  const lateSeen = [];
  emitter.event((value) => lateSeen.push(value));
  emitter.fire("five");
  check("subscribing to a disposed emitter is a no-op", lateSeen.length === 0);
}

if (failures.length > 0) {
  console.error(`lifecycle: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("lifecycle: all assertions passed");
