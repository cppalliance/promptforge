// The product's layout policy on the booted workbench (src/main.ts): with
// no stored layout, the dock takes #dock's size before the boot apply, and
// the policy the composition root registers seeds the Workshop tree into
// the left zone at 280px and the agent session into the right zone, with
// main empty. test/workshop-layout.mjs and test/layout-open-registry.mjs
// pin the boot decision against test policies; this proves the policy
// main.ts registers and the boot sizing it relies on.
// Run: node test/layout-policy-boot.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench(
  "the booted default layout puts the tree left at 280px and the agent right",
  async ({ resolveService, failures }) => {
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
    if (tree.group.api.width !== 280) {
      failures.push(`the tree's group is ${tree.group.api.width}px wide, expected 280`);
    }
    if (dock.groups.length !== 2) {
      failures.push(`the default layout built ${dock.groups.length} groups, expected 2 with main empty`);
    }
  },
  { dockSize: { width: 1200, height: 800 } },
);
