// Unit test for the recent-files store
// (src/services/recent-files-store.ts): the localStorage-backed list of
// recently opened paths feeding File > Open Recent and the quick-access
// "" provider. Bundles the module with esbuild and drives it with a fake
// Storage. Covers: most-recent-first order, dedupe on re-open, the
// retention cap, empty-path rejection, persistence across instances, the
// hand-written shape check on read (malformed JSON, non-array payloads,
// non-string and duplicate entries), clear semantics, the change event,
// and storage that is absent or throwing.
// Run: node --test test/recent-files-store.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

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

function fakeStorage(seed) {
  const map = new Map(Object.entries(seed ?? {}));
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => {
      map.set(key, String(value));
    },
    removeItem: (key) => {
      map.delete(key);
    },
    dump: () => Object.fromEntries(map),
  };
}

// --- Self-registration --------------------------------------------------------

check(
  "the RECENT_FILES_STORE token self-registers",
  getService(RECENT_FILES_STORE) instanceof RecentFilesStore,
);

// --- Ordering, dedupe, and the cap ----------------------------------------------

const storage = fakeStorage();
const store = new RecentFilesStore(storage, "test.recent");
check("a fresh store is empty", store.list.length === 0);

store.add("/a.ts");
store.add("/b.ts");
check("adds prepend, most recent first", store.list.join(",") === "/b.ts,/a.ts");

store.add("/a.ts");
check("re-adding moves the path to the front", store.list.join(",") === "/a.ts,/b.ts");
check("re-adding does not duplicate", store.list.length === 2);

store.add("");
check("an empty path is ignored", store.list.length === 2);

for (let i = 0; i < 55; i += 1) {
  store.add(`/f${i}.ts`);
}
check("the list is capped", store.list.length === 50);
check("the newest entry leads", store.list[0] === "/f54.ts");
check("the oldest entries drop off", !store.list.includes("/f4.ts") && store.list.includes("/f5.ts"));

// --- Persistence ------------------------------------------------------------------

const reloaded = new RecentFilesStore(storage, "test.recent");
check(
  "a second instance reads the persisted list",
  reloaded.list.join(",") === store.list.join(","),
);

// --- The shape check on read --------------------------------------------------------

check(
  "malformed JSON reads as empty",
  new RecentFilesStore(fakeStorage({ k: "not json" }), "k").list.length === 0,
);
check(
  "a non-array payload reads as empty",
  new RecentFilesStore(fakeStorage({ k: '{"a":1}' }), "k").list.length === 0,
);
check(
  "a bare string payload reads as empty",
  new RecentFilesStore(fakeStorage({ k: '"x"' }), "k").list.length === 0,
);
const mixed = new RecentFilesStore(fakeStorage({ k: '["ok",42,null,"also-ok",""]' }), "k");
check("non-string and empty entries drop out", mixed.list.join(",") === "ok,also-ok");
const duped = new RecentFilesStore(fakeStorage({ k: '["a","a","b"]' }), "k");
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
store.clear();
check("clearing an empty store does not fire", fires === 2);
const persisted = JSON.parse(storage.dump()["test.recent"]);
check("clear persists the empty list", Array.isArray(persisted) && persisted.length === 0);
subscription.dispose();
store.add("/after.ts");
check("a disposed subscription stops events", fires === 2);

// --- Absent and throwing storage ---------------------------------------------------------

const memoryOnly = new RecentFilesStore(null, "k");
memoryOnly.add("/x.ts");
check("a store without storage still tracks in memory", memoryOnly.list.length === 1);

const throwing = {
  getItem: () => {
    throw new Error("denied");
  },
  setItem: () => {
    throw new Error("denied");
  },
};
const degraded = new RecentFilesStore(throwing, "k");
check("a throwing read reads as empty", degraded.list.length === 0);
degraded.add("/y.ts");
check("a throwing write keeps the in-memory list", degraded.list.length === 1);

store.dispose();
reloaded.dispose();
memoryOnly.dispose();
degraded.dispose();

if (failures.length > 0) {
  console.error(`recent-files-store: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("recent-files-store: all assertions passed");
process.exit(0);
