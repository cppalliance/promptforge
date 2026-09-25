// Unit test for the closed-editor stack (src/parts/editor/closed-editors.ts,
// consumed by editor-lifecycle.ts): the adapter-backed stack behind
// Reopen Closed Editor. Bundles the module with esbuild and drives it
// over the fake UI-state adapter the way main.ts binds the live one: the
// initial value read from the workspace bucket's "closed_editors" key,
// every mutation written back to the same key as `{ paths: [...] }`,
// most recent first. Covers: the token's self-registration, the initial
// stack applied at construction (and construction writing nothing), push
// prepending the path in the written snapshot, pop removing it and
// writing, pop on an empty stack writing nothing, untitled entries kept
// in memory but absent from the persisted paths (unsaved text never
// persists), the retention cap of 50, replaceClosedEditors replacing the
// stack without a write, the hand-written shape check on the initial
// value, a new instance over the written value reading it back, and a
// writer that throws leaving the stack intact.
// Run: node --test test/closed-editors.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { createFakeUiStorage } from "./helpers/ui-storage.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { ClosedEditors } from "./src/parts/editor/closed-editors.ts";
      export { CLOSED_EDITORS } from "./src/services/closed-editors.ts";
      export { getService } from "./src/services/service-registry.ts";
    `,
    resolveDir: path.join(uiDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});
const { ClosedEditors, CLOSED_EDITORS, getService } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

/** The workspace-bucket key the composition root binds the stack to. */
const KEY = "closed_editors";

/**
 * Builds a stack over a fake adapter the way main.ts binds the live one.
 * Returns the stack and the fake, whose `sets` records the writes.
 */
function stackOver(initial) {
  const storage = createFakeUiStorage(initial === undefined ? {} : { workspace: { [KEY]: initial } });
  const stack = new ClosedEditors(storage.get("workspace", KEY), (value) =>
    storage.set("workspace", KEY, value),
  );
  return { stack, storage };
}

/** The last recorded write's value, or undefined when nothing was written. */
function lastWrite(storage) {
  return storage.sets[storage.sets.length - 1]?.value;
}

const file = (p) => ({ kind: "file", path: p });

// --- Self-registration --------------------------------------------------------

check("the CLOSED_EDITORS token self-registers", getService(CLOSED_EDITORS) instanceof ClosedEditors);
check("the default instance starts empty", getService(CLOSED_EDITORS).snapshot().paths.length === 0);
check("the default instance pops nothing", getService(CLOSED_EDITORS).pop() === undefined);

// --- The initial value ----------------------------------------------------------

{
  const { stack, storage } = stackOver({ paths: ["/b.ts", "/a.ts"] });
  check("the initial stack is applied most-recent-first", stack.snapshot().paths.join(",") === "/b.ts,/a.ts");
  check("construction writes nothing", storage.sets.length === 0);
  const first = stack.pop();
  check(
    "pop returns the most recently closed file first",
    first !== undefined && first.kind === "file" && first.path === "/b.ts",
  );
  const second = stack.pop();
  check("the next pop returns the older file", second !== undefined && second.path === "/a.ts");
  check("the seeded stack empties in order", stack.pop() === undefined);
}

// --- Push, pop, and the write-through -------------------------------------------

const { stack, storage } = stackOver();
check("a fresh stack is empty", stack.snapshot().paths.length === 0);

stack.push(file("/a.ts"));
check(
  "push writes the snapshot to the workspace bucket",
  storage.sets.length === 1 &&
    storage.sets[0].bucket === "workspace" &&
    storage.sets[0].key === KEY &&
    Array.isArray(storage.sets[0].value?.paths) &&
    storage.sets[0].value.paths.join(",") === "/a.ts",
);
stack.push(file("/b.ts"));
check("the snapshot lists the most recent close first", stack.snapshot().paths.join(",") === "/b.ts,/a.ts");
check("the second push writes the whole two-entry stack", lastWrite(storage)?.paths.join(",") === "/b.ts,/a.ts");

const popped = stack.pop();
check("pop returns the most recent close", popped?.kind === "file" && popped.path === "/b.ts");
check("pop writes the reduced stack", storage.sets.length === 3 && lastWrite(storage)?.paths.join(",") === "/a.ts");

stack.pop();
const writesBeforeEmptyPop = storage.sets.length;
check("popping the last entry writes the empty snapshot", lastWrite(storage)?.paths.length === 0);
check("pop on an empty stack returns undefined", stack.pop() === undefined);
check("pop on an empty stack writes nothing", storage.sets.length === writesBeforeEmptyPop);

// --- Untitled buffers stay in memory only ------------------------------------------

{
  const { stack: mixed, storage: mixedStorage } = stackOver();
  mixed.push(file("/x.ts"));
  mixed.push({ kind: "untitled", text: "draft" });
  check("an untitled close is absent from the persisted paths", lastWrite(mixedStorage)?.paths.join(",") === "/x.ts");
  check("an untitled close still writes a snapshot", mixedStorage.sets.length === 2);
  const untitled = mixed.pop();
  check(
    "an untitled close pops back with its text",
    untitled?.kind === "untitled" && untitled.text === "draft",
  );
  const next = mixed.pop();
  check("the file beneath an untitled close pops next", next?.kind === "file" && next.path === "/x.ts");
}

// --- The cap ----------------------------------------------------------------------------

{
  const { stack: capped, storage: cappedStorage } = stackOver();
  for (let i = 0; i < 55; i += 1) {
    capped.push(file(`/f${i}.ts`));
  }
  const paths = capped.snapshot().paths;
  check("the stack is capped at 50", paths.length === 50);
  check("the newest entry leads the snapshot", paths[0] === "/f54.ts");
  check("the oldest entries drop off", !paths.includes("/f4.ts") && paths.includes("/f5.ts"));
  check("the written snapshot is the capped stack", lastWrite(cappedStorage)?.paths.length === 50);
  const { stack: oversized } = stackOver({ paths: Array.from({ length: 60 }, (_, i) => `/p${i}.ts`) });
  const seeded = oversized.snapshot().paths;
  check("an oversized initial value keeps the 50 most recent", seeded.length === 50 && seeded[0] === "/p0.ts");
  check("an oversized initial value drops the oldest", !seeded.includes("/p50.ts") && seeded.includes("/p49.ts"));
}

// --- Persistence: a relaunch reads what the last write stored -----------------------

{
  const { stack: source, storage: sourceStorage } = stackOver();
  source.push(file("/one.ts"));
  source.push(file("/two.ts"));
  const reloaded = new ClosedEditors(sourceStorage.get("workspace", KEY), () => {});
  check(
    "a new stack over the written value reads it back",
    reloaded.snapshot().paths.join(",") === source.snapshot().paths.join(","),
  );
}

// --- replaceClosedEditors (Open Workspace from File) ----------------------------------

{
  const { stack: switched, storage: switchedStorage } = stackOver({ paths: ["/old.ts"] });
  switched.push(file("/live.ts"));
  const writesBeforeReplace = switchedStorage.sets.length;
  switched.replaceClosedEditors(["/new-b.ts", "/new-a.ts"]);
  check("replaceClosedEditors replaces the stack", switched.snapshot().paths.join(",") === "/new-b.ts,/new-a.ts");
  check("replaceClosedEditors writes nothing", switchedStorage.sets.length === writesBeforeReplace);
  const top = switched.pop();
  check("the replaced stack pops most-recent-first", top?.kind === "file" && top.path === "/new-b.ts");
  switched.replaceClosedEditors([]);
  check("replacing with an empty list empties the stack", switched.pop() === undefined);
}

// --- The shape check on the initial value --------------------------------------------

check("a null initial value reads as empty", stackOver(null).stack.snapshot().paths.length === 0);
check("a string initial value reads as empty, never a cast", stackOver("nope").stack.snapshot().paths.length === 0);
check("a bare array reads as empty", stackOver(["/a.ts"]).stack.snapshot().paths.length === 0);
check("an object without paths reads as empty", stackOver({ other: 1 }).stack.snapshot().paths.length === 0);
check("a non-array paths field reads as empty", stackOver({ paths: "x" }).stack.snapshot().paths.length === 0);
check(
  "non-string and empty entries drop out",
  stackOver({ paths: ["ok", 42, null, "", "also"] }).stack.snapshot().paths.join(",") === "ok,also",
);

// --- A writer that fails -----------------------------------------------------------------

{
  const memoryOnly = new ClosedEditors();
  memoryOnly.push(file("/x.ts"));
  check("a stack with the default no-op writer still tracks in memory", memoryOnly.snapshot().paths.length === 1);

  let attempts = 0;
  const degraded = new ClosedEditors({ paths: ["/kept.ts"] }, () => {
    attempts += 1;
    throw new Error("denied");
  });
  degraded.push(file("/y.ts"));
  check("a throwing writer is still called", attempts === 1);
  check("a throwing write keeps the in-memory stack", degraded.snapshot().paths.join(",") === "/y.ts,/kept.ts");
  const popped = degraded.pop();
  check("a throwing write on pop still pops", popped?.path === "/y.ts" && attempts === 2);
}

if (failures.length > 0) {
  console.error(`closed-editors: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("closed-editors: all assertions passed");
process.exit(0);
