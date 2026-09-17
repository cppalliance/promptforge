import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { UpdateService } from "./src/services/update-service.ts";
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
const code = bundle.outputFiles[0].text;
const { lifecycle, UpdateService } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
);

const failures = [];
const check = (name, condition) => {
  if (!condition) failures.push(name);
};

await assertNoLeaks(lifecycle, async () => {
  let relaunched = 0;
  let closed = 0;
  const update = {
    currentVersion: "0.2.0",
    version: "0.3.0",
    body: "Faster startup\nMore details",
    async downloadAndInstall(onEvent) {
      onEvent({ event: "Started", data: { contentLength: 100 } });
      onEvent({ event: "Progress", data: { chunkLength: 40 } });
      onEvent({ event: "Progress", data: { chunkLength: 60 } });
      onEvent({ event: "Finished" });
    },
    async close() {
      closed += 1;
    },
  };
  const service = new UpdateService({
    desktop: true,
    supported: async () => true,
    currentVersion: async () => "0.2.0",
    check: async () => update,
    relaunch: async () => {
      relaunched += 1;
    },
  });
  await service.checkNow();
  check("a newer release becomes available", service.snapshot.phase === "available");
  check("the available version is retained", service.snapshot.version === "0.3.0");
  check("only the first release-note line is shown", service.snapshot.notes === "Faster startup");
  service.remindLater();
  check("remind later dismisses the banner state", service.snapshot.phase === "dismissed");
  service.showAvailable();
  check("a dismissed update can be shown again", service.snapshot.phase === "available");
  await service.install();
  check("the completed update reaches restart", service.snapshot.phase === "restarting");
  check("download chunks accumulate against the total", service.snapshot.downloaded === 100);
  check("the process relaunch is requested", relaunched === 1);
  service.dispose();
  await Promise.resolve();
  check("disposing closes the native update handle", closed === 1);

  let checked = 0;
  const deb = new UpdateService({
    desktop: true,
    supported: async () => false,
    currentVersion: async () => "0.2.0",
    check: async () => {
      checked += 1;
      return null;
    },
    relaunch: async () => undefined,
  });
  await deb.checkNow();
  check("package-managed Linux disables in-app updates", deb.snapshot.phase === "unsupported");
  check("an unsupported package never checks the updater endpoint", checked === 0);
  deb.dispose();
});

if (failures.length) {
  console.error("Update service failures:\n" + failures.map((failure) => `  - ${failure}`).join("\n"));
  process.exitCode = 1;
} else {
  console.log("Update service checks passed.");
}
