// Guard for the UI-state migration (plan step 13): the SPA keeps no
// browser-storage state. The desktop app binds the server to an OS-assigned
// loopback port, so the page origin - and every origin-scoped storage
// entry with it - changes on each launch; every store now reads and
// writes through the server-backed UI-state adapter instead. Walks every
// file under src/ and fails on any that names the browser storage API,
// so a store cannot quietly grow a per-origin fallback again.
// Run: node --test test/no-local-storage.mjs
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const srcDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "src");

// Spelled as a pattern so this guard's own source, were it ever moved
// under src/, would still describe rather than trip the rule.
const FORBIDDEN = new RegExp(["local", "Storage"].join(""));

const files = (await readdir(srcDir, { recursive: true, withFileTypes: true }))
  .filter((entry) => entry.isFile())
  .map((entry) => path.join(entry.parentPath ?? entry.path, entry.name))
  .sort();

if (files.length === 0) {
  console.error("no-local-storage: src/ holds no files; the walk is broken");
  process.exit(1);
}

const offenders = [];
for (const file of files) {
  const text = await readFile(file, "utf8");
  const lines = text.split("\n");
  lines.forEach((line, index) => {
    if (FORBIDDEN.test(line)) {
      offenders.push(`${path.relative(srcDir, file)}:${index + 1}: ${line.trim()}`);
    }
  });
}

if (offenders.length > 0) {
  console.error(`no-local-storage: ${offenders.length} reference(s) to browser storage under src/`);
  for (const offender of offenders) console.error(`  - ${offender}`);
  process.exit(1);
}
console.log(`no-local-storage: ${files.length} files under src/ name no browser storage`);
