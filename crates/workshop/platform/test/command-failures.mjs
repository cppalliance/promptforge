// Unit test for the command registry's failure report (command-registry.ts):
// a command that rejects or throws fires onDidFailCommand with its id, its
// palette label (Category: Title, the bare title, or the id), and the error,
// and execute still rejects with that same error so a caller that awaits the
// run keeps its own handling. A command that succeeds, an unregistered id, and
// a disposed subscription report nothing. The UI turns the report into the
// "Command '{0}' resulted in an error" toast; the platform stays DOM-free.
// Run: node --test test/command-failures.mjs (from crates/workshop/platform).
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const platformDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const bundle = await esbuild.build({
  stdin: {
    contents: `export { CommandRegistry } from "./command-registry.ts";`,
    resolveDir: platformDir,
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});
const { CommandRegistry } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

/** Runs `id` and answers the rejection, or "resolved" when it did not reject. */
async function settle(commands, id) {
  try {
    await commands.execute(id);
    return "resolved";
  } catch (error) {
    return error;
  }
}

{
  // A rejection reports once, with the palette label, and still rejects.
  const commands = new CommandRegistry();
  const boom = new Error("disk full");
  commands.register("file.save", { title: "Save", category: "File", run: () => Promise.reject(boom) });
  const reports = [];
  commands.onDidFailCommand((failure) => reports.push(failure));
  const outcome = await settle(commands, "file.save");
  check("execute still rejects with the command's own error", outcome === boom);
  check("a rejection reports exactly once", reports.length === 1);
  check("the report names the command id", reports[0]?.id === "file.save");
  check("the report label is Category: Title", reports[0]?.label === "File: Save");
  check("the report carries the error", reports[0]?.error === boom);
}

{
  // A synchronous throw reports the same way, and the label falls back.
  const commands = new CommandRegistry();
  const crash = new TypeError("not a function");
  commands.register("window.reload", {
    title: "Reload Window",
    run: () => {
      throw crash;
    },
  });
  commands.register("bare.command", {
    run: () => {
      throw crash;
    },
  });
  const reports = [];
  commands.onDidFailCommand((failure) => reports.push(failure));
  const outcome = await settle(commands, "window.reload");
  check("a synchronous throw rejects execute with the same error", outcome === crash);
  check("a category-less label is the bare title", reports[0]?.label === "Reload Window");
  await settle(commands, "bare.command");
  check("a command without a title labels with its id", reports[1]?.label === "bare.command");
}

{
  // Success, an unknown id, and a disposed subscription report nothing.
  const commands = new CommandRegistry();
  commands.register("ok", { title: "Fine", run: () => {} });
  commands.register("bad", { title: "Bad", run: () => Promise.reject(new Error("no")) });
  const reports = [];
  const subscription = commands.onDidFailCommand((failure) => reports.push(failure));
  check("a successful command resolves true", (await commands.execute("ok")) === true);
  check("an unregistered id resolves false", (await commands.execute("missing")) === false);
  check("neither reports a failure", reports.length === 0);
  subscription.dispose();
  await settle(commands, "bad");
  check("a disposed subscription hears nothing", reports.length === 0);
}

{
  // labelOf answers the same label a failure reports, for a run that bypassed the registry.
  const commands = new CommandRegistry();
  commands.register("file.save", { title: "Save", category: "File", run: () => {} });
  commands.register("window.reload", { title: "Reload Window", run: () => {} });
  commands.register("bare.command", { run: () => {} });
  check("labelOf is Category: Title", commands.labelOf("file.save") === "File: Save");
  check("labelOf is the bare title without a category", commands.labelOf("window.reload") === "Reload Window");
  check("labelOf is the id without a title", commands.labelOf("bare.command") === "bare.command");
  check("labelOf is the id for an unregistered command", commands.labelOf("missing.command") === "missing.command");
}

{
  // Two registries report independently: a test registry never feeds the shared one.
  const first = new CommandRegistry();
  const second = new CommandRegistry();
  first.register("bad", { title: "Bad", run: () => Promise.reject(new Error("no")) });
  const heard = [];
  second.onDidFailCommand((failure) => heard.push(failure));
  await settle(first, "bad");
  check("a failure reports only on the registry that ran it", heard.length === 0);
}

if (failures.length > 0) {
  console.error(`command-failures: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("command-failures: all assertions passed");
