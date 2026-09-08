import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const [
  installer,
  mainWorkflow,
  installerWorkflow,
  nightlyConfigSource,
] = await Promise.all([
  readFile(
    new URL("../crates/workshop/installer.nsi", import.meta.url),
    "utf8",
  ),
  readFile(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8"),
  readFile(
    new URL(
      "../.github/workflows/workshop-installer-smoke.yml",
      import.meta.url,
    ),
    "utf8",
  ),
  readFile(
    new URL("../crates/workshop/tauri.nightly.conf.json", import.meta.url),
    "utf8",
  ),
]);

const CALLBACK = "!define MUI_PAGE_CUSTOMFUNCTION_SHOW FinishPageShow";
const FINISH_PAGE = "!insertmacro MUI_PAGE_FINISH";
const TEXT_COLOR = '!define MUI_TEXTCOLOR "E0E0E0"';
const WORKSHOP_BRANCH =
  '${If} ${FileExists} "$INSTDIR\\${MAINBINARYNAME}.exe"';
const GATEWAY_BRANCH =
  '${ElseIf} ${FileExists} "$INSTDIR\\promptforge-gateway.exe"';

function normalizedLines(source) {
  return source.replaceAll("\r\n", "\n").split("\n");
}

function exactLineIndexes(lines, expected) {
  const indexes = [];
  for (const [index, line] of lines.entries()) {
    if (line.trim() === expected) {
      indexes.push(index);
    }
  }
  return indexes;
}

function functionBody(lines, name) {
  const starts = exactLineIndexes(lines, `Function ${name}`);
  assert.equal(starts.length, 1, `${name} exists exactly once`);
  const end = lines.findIndex(
    (line, index) => index > starts[0] && line.trim() === "FunctionEnd",
  );
  assert.notEqual(end, -1, `${name} has a FunctionEnd`);
  return lines.slice(starts[0] + 1, end);
}

export function assertWorkshopInstallerTheme(source) {
  const lines = normalizedLines(source);
  const callback = exactLineIndexes(lines, CALLBACK);
  const finishPage = exactLineIndexes(lines, FINISH_PAGE);

  assert.equal(
    callback.length,
    1,
    "the finish page must bind FinishPageShow exactly once",
  );
  assert.equal(
    finishPage.length,
    1,
    "the finish page must be declared exactly once",
  );
  assert.ok(
    callback[0] < finishPage[0],
    "the FinishPageShow callback binding must precede the finish page declaration",
  );
  assert.equal(
    finishPage[0],
    callback[0] + 1,
    "the FinishPageShow callback binding must immediately govern the finish page declaration",
  );
  assert.equal(
    exactLineIndexes(lines, TEXT_COLOR).length,
    1,
    "MUI_TEXTCOLOR must remain E0E0E0",
  );

  const finish = functionBody(lines, "FinishPageShow");
  const componentBranch = exactLineIndexes(finish, WORKSHOP_BRANCH);
  assert.equal(
    componentBranch.length,
    1,
    "FinishPageShow must contain the component-dependent branch",
  );

  for (const control of [
    "$mui.FinishPage.Run",
    "$mui.FinishPage.ShowReadme",
  ]) {
    const theme = `System::Call 'UXTHEME::SetWindowTheme(p${control},w" ",w" ")'`;
    const colors = `SetCtlColors ${control} "\${MUI_TEXTCOLOR}" "\${MUI_BGCOLOR}"`;
    const themeIndexes = exactLineIndexes(finish, theme);
    const colorIndexes = exactLineIndexes(finish, colors);
    assert.equal(
      themeIndexes.length,
      1,
      `${control} disables visual-style color override exactly once`,
    );
    assert.equal(
      colorIndexes.length,
      1,
      `${control} reapplies MUI theme colors exactly once`,
    );
    assert.ok(
      themeIndexes[0] < colorIndexes[0],
      `${control} disables visual styles before setting colors`,
    );
    assert.ok(
      themeIndexes[0] < componentBranch[0] &&
        colorIndexes[0] < componentBranch[0],
      `${control} theme and color repair must precede the component-dependent branch`,
    );
  }

  assert.match(
    finish.join("\n"),
    /https:\/\/sourceforge\.net\/p\/nsis\/bugs\/443\//,
    "the upstream NSIS visual-style defect citation must remain",
  );
}

function replaceExactlyOnce(source, before, after) {
  const first = source.indexOf(before);
  assert.notEqual(first, -1, `synthetic fixture contains ${before}`);
  assert.equal(
    source.indexOf(before, first + before.length),
    -1,
    `synthetic fixture contains ${before} exactly once`,
  );
  return source.slice(0, first) + after + source.slice(first + before.length);
}

function moveControlRepairUnder(source, control, branch) {
  const lines = normalizedLines(source);
  const theme =
    `System::Call 'UXTHEME::SetWindowTheme(p${control},w" ",w" ")'`;
  const colors =
    `SetCtlColors ${control} "\${MUI_TEXTCOLOR}" "\${MUI_BGCOLOR}"`;
  const themeIndex = exactLineIndexes(lines, theme);
  const colorIndex = exactLineIndexes(lines, colors);
  assert.equal(themeIndex.length, 1);
  assert.equal(colorIndex.length, 1);
  assert.equal(colorIndex[0], themeIndex[0] + 1);
  const repair = lines.splice(themeIndex[0], 2);
  const finishStart = exactLineIndexes(lines, "Function FinishPageShow");
  assert.equal(finishStart.length, 1);
  const finishEnd = lines.findIndex(
    (line, index) =>
      index > finishStart[0] && line.trim() === "FunctionEnd",
  );
  assert.notEqual(finishEnd, -1);
  const branchIndex = exactLineIndexes(lines, branch).filter(
    (index) => index > finishStart[0] && index < finishEnd,
  );
  assert.equal(branchIndex.length, 1);
  lines.splice(branchIndex[0] + 1, 0, ...repair);
  return lines.join("\n");
}

function workflowJobSource(source, name) {
  const normalized = source.replaceAll("\r\n", "\n");
  const marker = `  ${name}:\n`;
  const start = normalized.indexOf(marker);
  assert.notEqual(start, -1, `workflow contains ${name}`);
  const remainder = normalized.slice(start + marker.length);
  const nextJob = remainder.search(/^  [A-Za-z0-9_-]+:\n/m);
  return normalized.slice(
    start,
    nextJob === -1 ? normalized.length : start + marker.length + nextJob,
  );
}

test("finish-page checkboxes remain legible on the dark theme", () => {
  assertWorkshopInstallerTheme(installer);
});

test("finish-page assertion rejects a detached callback", () => {
  const detached = replaceExactlyOnce(
    installer,
    CALLBACK,
    "!define MUI_PAGE_CUSTOMFUNCTION_SHOW DetachedFinishPageShow",
  );
  assert.throws(
    () => assertWorkshopInstallerTheme(detached),
    /must bind FinishPageShow exactly once/,
  );
});

test("finish-page assertion rejects reversed callback declaration order", () => {
  const reversed = replaceExactlyOnce(
    installer.replaceAll("\r\n", "\n"),
    `${CALLBACK}\n${FINISH_PAGE}`,
    `${FINISH_PAGE}\n${CALLBACK}`,
  );
  assert.throws(
    () => assertWorkshopInstallerTheme(reversed),
    /callback binding must precede the finish page declaration/,
  );
});

for (const { branch, control, component } of [
  {
    branch: WORKSHOP_BRANCH,
    control: "$mui.FinishPage.Run",
    component: "Workshop",
  },
  {
    branch: GATEWAY_BRANCH,
    control: "$mui.FinishPage.ShowReadme",
    component: "Gateway",
  },
]) {
  test(`finish-page assertion rejects ${control} repair inside the ${component} branch`, () => {
    const scoped = moveControlRepairUnder(installer, control, branch);
    assert.throws(
      () => assertWorkshopInstallerTheme(scoped),
      new RegExp(
        `${control.replaceAll("$", "\\$")} theme and color repair must precede`,
      ),
    );
  });
}

test("finish-page assertion requires the upstream NSIS bug citation", () => {
  const uncited = replaceExactlyOnce(
    installer,
    "https://sourceforge.net/p/nsis/bugs/443/",
    "https://example.invalid/nsis-visual-style-bug/",
  );
  assert.throws(
    () => assertWorkshopInstallerTheme(uncited),
    /defect citation must remain/,
  );
});

test("main CI keeps the fast textual installer check", () => {
  assert.match(
    mainWorkflow,
    /node --test tools\/check-workshop-installer-theme\.test\.mjs/,
  );
  assert.doesNotMatch(
    workflowJobSource(mainWorkflow, "check-workshop"),
    /NSIS installer/,
  );
});

test("path-gated Windows CI compiles an unsigned debug NSIS installer", () => {
  assert.match(installerWorkflow, /^  pull_request:\n/m);
  assert.match(installerWorkflow, /^  workflow_dispatch:\n/m);
  for (const sensitivePath of [
    ".github/workflows/workshop-installer-smoke.yml",
    "crates/build-workshop/**",
    "crates/workshop/installer.nsi",
    "crates/workshop/tauri*.conf.json",
    "tools/stage-gateway-sidecar.mjs",
  ]) {
    assert.ok(
      normalizedLines(installerWorkflow).some(
        (line) => line.trim() === `- ${sensitivePath}`,
      ),
      `installer smoke includes path filter ${sensitivePath}`,
    );
  }

  const workshopJob = workflowJobSource(
    installerWorkflow,
    "compile-nsis",
  );
  const stage = workshopJob.indexOf("- name: Stage Gateway sidecar");
  const smoke = workshopJob.indexOf(
    "- name: Compile unsigned debug NSIS installer",
  );
  const cleanup = workshopJob.indexOf("- name: Remove Gateway sidecar");

  assert.match(workshopJob, /runs-on: windows-latest/);
  assert.ok(stage > 0, "installer CI stages the Gateway sidecar");
  assert.match(
    workshopJob.slice(stage, smoke),
    /target\/debug\/promptforge-gateway\.exe/,
    "installer CI stages the debug Gateway",
  );
  assert.ok(smoke > stage, "NSIS smoke runs after Gateway staging");
  assert.ok(cleanup > smoke, "Gateway cleanup runs after the NSIS smoke");
  assert.match(
    workshopJob.slice(smoke, cleanup),
    /uses: tauri-apps\/tauri-action@v0/,
  );
  assert.match(
    workshopJob.slice(smoke, cleanup),
    /projectPath: crates\/workshop/,
  );
  assert.match(
    workshopJob.slice(smoke, cleanup),
    /args: --debug --bundles nsis --config tauri\.nightly\.conf\.json/,
  );
  assert.doesNotMatch(
    workshopJob.slice(smoke, cleanup),
    /TAURI_SIGNING_PRIVATE_KEY/,
  );
  assert.match(
    workshopJob.slice(cleanup),
    /if: always\(\)/,
    "staged Gateway cleanup remains unconditional",
  );
  assert.equal(
    JSON.parse(nightlyConfigSource).bundle.createUpdaterArtifacts,
    false,
    "the smoke config disables signed updater artifacts",
  );
});

test("installer names components in operator-facing order", () => {
  const components = [...installer.matchAll(
    /^Section "([^"]+)" Sec(Workshop|Gateway|STT)$/gm,
  )].map((match) => [match[1], match[2]]);
  assert.deepEqual(components, [
    ["PromptForge Workshop", "Workshop"],
    ["PromptForge Gateway", "Gateway"],
    ["Speech to Text (Transcription)", "STT"],
  ]);
});
