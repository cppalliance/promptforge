// Unit test for the recent-files store
// (src/services/recent-files-store.ts): the adapter-backed list of
// recently opened paths feeding File > Open Recent and the quick-access
// "" provider. Bundles the module with esbuild and drives it over the
// fake UI-state adapter the way main.ts binds the live one: the initial
// value read from the user bucket, every change written back to the same
// key. Covers: the initial list applied at construction, most-recent-first
// order, dedupe on re-open, the retention cap, empty-path rejection, each
// mutation writing the full list, a new instance over the written value
// reading it back, the hand-written shape check on the initial value
// (non-array payloads, non-string and duplicate entries), clear
// semantics, the change event, and a writer that throws leaving the
// in-memory list intact.
// Run: node --test test/recent-files-store.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { createFakeUiStorage } from "./helpers/ui-storage.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { RecentFilesStore, RECENT_FILES_STORE } from "./src/services/recent-files-store.ts";
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
const { RecentFilesStore, RECENT_FILES_STORE, getService } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

/** The user-bucket key the composition root binds the store to. */
const KEY = "recent_files";

/**
 * Builds a store over a fake adapter the way main.ts binds the live one.
 * Returns the store and the fake, whose `sets` records the writes.
 */
function storeOver(initial) {
  const storage = createFakeUiStorage(initial === undefined ? {} : { user: { [KEY]: initial } });
  const store = new RecentFilesStore(storage.get("user", KEY), (value) => storage.set("user", KEY, value));
  return { store, storage };
}

/** The last recorded write's value, or undefined when nothing was written. */
function lastWrite(storage) {
  return storage.sets[storage.sets.length - 1]?.value;
}

// --- Self-registration --------------------------------------------------------

check(
  "the RECENT_FILES_STORE token self-registers",
  getService(RECENT_FILES_STORE) instanceof RecentFilesStore,
);
check("the default instance starts empty", getService(RECENT_FILES_STORE).list.length === 0);

// --- The initial value ----------------------------------------------------------

{
  const { store, storage } = storeOver(["/one.ts", "/two.ts"]);
  check("the initial list is applied most-recent-first", store.list.join(",") === "/one.ts,/two.ts");
  check("construction writes nothing", storage.sets.length === 0);
  store.dispose();
}

// --- Ordering, dedupe, the cap, and the write-through ---------------------------

const { store, storage } = storeOver();
check("a fresh store is empty", store.list.length === 0);

store.add("/a.ts");
check(
  "add writes the full list to the user bucket",
  storage.sets.length === 1 &&
    storage.sets[0].bucket === "user" &&
    storage.sets[0].key === KEY &&
    Array.isArray(storage.sets[0].value) &&
    storage.sets[0].value.join(",") === "/a.ts",
);
store.add("/b.ts");
check("adds prepend, most recent first", store.list.join(",") === "/b.ts,/a.ts");
check("the second add writes the whole two-entry list", lastWrite(storage)?.join(",") === "/b.ts,/a.ts");

store.add("/a.ts");
check("re-adding moves the path to the front", store.list.join(",") === "/a.ts,/b.ts");
check("re-adding does not duplicate", store.list.length === 2);
check("re-adding writes the reordered list", lastWrite(storage)?.join(",") === "/a.ts,/b.ts");

const writesBeforeEmpty = storage.sets.length;
store.add("");
check("an empty path is ignored", store.list.length === 2);
check("an ignored add writes nothing", storage.sets.length === writesBeforeEmpty);

for (let i = 0; i < 55; i += 1) {
  store.add(`/f${i}.ts`);
}
check("the list is capped", store.list.length === 50);
check("the newest entry leads", store.list[0] === "/f54.ts");
check("the oldest entries drop off", !store.list.includes("/f4.ts") && store.list.includes("/f5.ts"));
check("the written list is the capped list", lastWrite(storage)?.length === 50 && lastWrite(storage)[0] === "/f54.ts");

// --- Persistence: a relaunch reads what the last write stored -----------------------

{
  const reloaded = new RecentFilesStore(storage.get("user", KEY), () => {});
  check("a new store over the written value reads it back", reloaded.list.join(",") === store.list.join(","));
  reloaded.dispose();
}

// --- The shape check on the initial value --------------------------------------------

check("a null initial value reads as empty", storeOver(null).store.list.length === 0);
check("a string initial value reads as empty, never a cast", storeOver("not a list").store.list.length === 0);
check("a non-array object reads as empty", storeOver({ a: 1 }).store.list.length === 0);
check("a number reads as empty", storeOver(42).store.list.length === 0);
const { store: mixed } = storeOver(["ok", 42, null, "also-ok", ""]);
check("non-string and empty entries drop out", mixed.list.join(",") === "ok,also-ok");
const { store: duped } = storeOver(["a", "a", "b"]);
check("persisted duplicates collapse", duped.list.join(",") === "a,b");

// --- Clear and the change event -------------------------------------------------------

let fires = 0;
const subscription = store.onDidChange(() => {
  fires += 1;
});
store.add("/new.ts");
check("add fires onDidChange", fires === 1);
store.clear();
check("clear fires onDidChange", fires === 2);
check("clear empties the list", store.list.length === 0);
check("clear writes the empty list", Array.isArray(lastWrite(storage)) && lastWrite(storage).length === 0);
const writesBeforeNoop = storage.sets.length;
store.clear();
check("clearing an empty store does not fire", fires === 2);
check("clearing an empty store writes nothing", storage.sets.length === writesBeforeNoop);
subscription.dispose();
store.add("/after.ts");
check("a disposed subscription stops events", fires === 2);

// --- A writer that fails -----------------------------------------------------------------

{
  const memoryOnly = new RecentFilesStore();
  memoryOnly.add("/x.ts");
  check("a store with the default no-op writer still tracks in memory", memoryOnly.list.length === 1);
  memoryOnly.dispose();

  let attempts = 0;
  const degraded = new RecentFilesStore(["/kept.ts"], () => {
    attempts += 1;
    throw new Error("denied");
  });
  let fired = 0;
  degraded.onDidChange(() => {
    fired += 1;
  });
  degraded.add("/y.ts");
  check("a throwing writer is still called", attempts === 1);
  check("a throwing write keeps the in-memory list", degraded.list.join(",") === "/y.ts,/kept.ts");
  check("a throwing write still fires onDidChange", fired === 1);
  degraded.clear();
  check("a throwing write on clear still empties the list", degraded.list.length === 0 && attempts === 2);
  degraded.dispose();
}

store.dispose();
mixed.dispose();
duped.dispose();

if (failures.length > 0) {
  console.error(`recent-files-store: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("recent-files-store: all assertions passed");
process.exit(0);
