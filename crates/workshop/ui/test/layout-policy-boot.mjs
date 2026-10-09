// The product's layout policy on the booted workbench (src/main.ts): with
// no stored layout, the dock takes #dock's size before the boot apply, and
// the policy the composition root registers seeds the Workshop tree into
// the left zone at min(300px, W/4) (never under 214px) and the agent
// session into the right zone at min(400px, W/4) (never under 300px), with
// an empty editor group between them from the first paint - the watermark's
// home. The dock draws Cursor's 1px borders between its parts.
// test/workshop-layout.mjs and test/layout-open-registry.mjs pin the boot
// decision against test policies; this proves the policy main.ts registers,
// the dock options it passes, and the boot sizing it relies on. It also
// closes the tree and grants a folder, to prove the watermark's "Add Folder"
// row follows a workspace change with no tree panel alive.
// Run: node test/layout-policy-boot.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench(
  "the booted default layout puts the tree left, an empty editor group in the middle, and the agent right",
  async ({ document, window, resolveService, sleep, failures }) => {
    const dock = resolveService("workshop.dock");
    const zones = resolveService("workshop.zoneState");
    const tree = dock.getPanel("tree");
    const agent = dock.getPanel("agent");
    if (!tree || !agent) {
      failures.push("the default layout opened no tree or no agent panel");
      return;
    }
    const treeZone = zones.zoneForGroupId(tree.group.id);
    const agentZone = zones.zoneForGroupId(agent.group.id);
    if (treeZone !== "left" || agentZone !== "right") {
      failures.push(`the tree opened in ${treeZone} and the agent in ${agentZone}, expected left and right`);
    }
    // A 1200px dock: a quarter is 300px, which is the tree's preferred width
    // (min(300, 300)) and the agent's floor (max(300, min(400, 300))).
    if (tree.group.api.width !== 300) {
      failures.push(`the tree's group is ${tree.group.api.width}px wide, expected 300`);
    }
    if (agent.group.api.width !== 300) {
      failures.push(`the agent's group is ${agent.group.api.width}px wide, expected 300`);
    }
    if (dock.groups.length !== 3) {
      failures.push(`the default layout built ${dock.groups.length} groups, expected 3 with main empty`);
    }
    const mainId = zones.groupFor("main");
    const main = mainId === undefined ? undefined : dock.getGroup(mainId);
    if (main === undefined) {
      failures.push("the default layout has no main-zone group");
      return;
    }
    if (main.panels.length !== 0) {
      failures.push(`the main group holds ${main.panels.length} panels at startup, expected none`);
    }
    if (main.element.dataset.wsEmpty !== "true") {
      failures.push("the empty main group is not marked empty");
    }
    if (main.element.dataset.wsActiveGroup !== "true") {
      failures.push("the empty main group is not the Workshop's active group at startup");
    }
    if (main.api.width !== 600) {
      failures.push(`the empty main group is ${main.api.width}px wide, expected the 600px between the sides`);
    }

    // The watermark fills the empty group and only that group.
    const watermark = main.element.querySelector(".ws-watermark");
    if (watermark === null) {
      failures.push("the empty main group shows no watermark");
    } else {
      const labels = [...watermark.querySelectorAll(".ws-watermark__label")].map((label) => label.textContent);
      // The boot's roots listing is empty, so no Add Folder row.
      if (labels.join("|") !== "New Agent|Show Files|Search Files") {
        failures.push(`the watermark rows read ${labels.join("|")}, expected New Agent|Show Files|Search Files`);
      }
    }

    // hideBorders: false gives the split view Dockview's separator-border class.
    if (document.querySelector("#dock .dv-split-view-container.dv-separator-border") === null) {
      failures.push("the dock's split view carries no separator border");
    }

    // The watermark is on screen when Explorer is closed, and then no tree panel is
    // alive to drop the shared roots listing. Granting a folder must still reach it:
    // the composition root has the tree-state service follow the workspace-changed
    // event, so the listing is dropped and the "Add Folder" row (one root) appears.
    dock.removePanel(tree);
    await sleep(50);
    const bootFetch = globalThis.fetch;
    const grantedRoot = { name: "project", path: "C:\\project", kind: "directory", size: 0, modified_ms: 1, exists: true };
    globalThis.fetch = (url, init) =>
      url === "/workspace/tree"
        ? Promise.resolve(
            new Response(JSON.stringify({ path: null, entries: [grantedRoot] }), {
              status: 200,
              headers: { "content-type": "application/json" },
            }),
          )
        : bootFetch(url, init);
    try {
      window.dispatchEvent(new window.CustomEvent("promptforge:workspace-changed"));
      await sleep(100);
    } finally {
      globalThis.fetch = bootFetch;
    }
    const labelsAfterGrant = [...main.element.querySelectorAll(".ws-watermark__label")].map((label) => label.textContent);
    if (labelsAfterGrant.join("|") !== "New Agent|Show Files|Search Files|Add Folder") {
      failures.push(
        `with no tree panel open, granting a folder left the watermark rows at ${labelsAfterGrant.join("|")}, expected Add Folder to appear`,
      );
    }
  },
  { dockSize: { width: 1200, height: 800 } },
);
