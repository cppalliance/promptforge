// Source-text contract for the dock's and the file tree's stylesheets
// (src/parts/layout/zones.css, src/parts/workspace/workshop-panel.css).
// jsdom applies no layout, so the values are read from the files and followed
// through the token sheets (test/helpers/css-values.mjs) down to the literals
// Cursor's workbench uses. Covers: the dockview palette (groups #181818, the
// tab strip #141414, active tab #181818 and inactive #141414, tab text #F0F0F0
// and #F0F0F05C with #F0F0F0BD in unfocused groups, separators #F0F0F013, the
// drop overlay #F0F0F011, the sash lighting #F0F0F026 after 300ms over 100ms);
// the editor tabs (35px tall, 10px left padding, at least 120px wide, a 1px
// right border and strip underline, the active tab's #181818 underline, the
// hover wash only in unfocused groups); the close button (20x20 in a 28px
// slot, a 5px radius, hidden until the tab is hovered or active, half visible
// outside the active group, a dot in its place on a dirty tab until hover);
// the empty editor group (strip hidden, #141414); the left zone's pills; and
// the tree (full-width rows driven by a depth variable, the twistie box, the
// indent guides, the section header with hover actions, the drag pill).
// Run: node test/dock-css.mjs
import { declaration, readUi, resolver, rulesOf, valueIn } from "./helpers/css-values.mjs";

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const resolve = await resolver();
const zones = rulesOf(await readUi("src/parts/layout/zones.css"));
const tree = rulesOf(await readUi("src/parts/workspace/workshop-panel.css"));
const zone = (selector, property, at = null) => resolve(valueIn(zones, selector, property, at));
const treeValue = (selector, property, at = null) => resolve(valueIn(tree, selector, property, at));

// --- The dockview palette ---------------------------------------------------------------

// Dockview puts the theme class on its shell element, inside the #dock element.
const DOCK = ".ws-dock .dockview-theme-dark";
check("groups are #181818", zone(DOCK, "--dv-group-view-background-color") === "#181818");
check("the tab strip is #141414", zone(DOCK, "--dv-tabs-and-actions-container-background-color") === "#141414");
check("the strip is 35px tall", zone(DOCK, "--dv-tabs-and-actions-container-height") === "35px");
check(
  "the active tab is #181818 in every group state",
  zone(DOCK, "--dv-activegroup-visiblepanel-tab-background-color") === "#181818" &&
    zone(DOCK, "--dv-inactivegroup-visiblepanel-tab-background-color") === "#181818",
);
check(
  "an inactive tab is #141414 in every group state",
  zone(DOCK, "--dv-activegroup-hiddenpanel-tab-background-color") === "#141414" &&
    zone(DOCK, "--dv-inactivegroup-hiddenpanel-tab-background-color") === "#141414",
);
check(
  "an inactive tab's text is #F0F0F05C",
  zone(DOCK, "--dv-activegroup-hiddenpanel-tab-color") === "#f0f0f05c" &&
    zone(DOCK, "--dv-inactivegroup-hiddenpanel-tab-color") === "#f0f0f05c",
);
check(
  "an unfocused group's active tab text is #F0F0F0BD",
  zone(DOCK, "--dv-activegroup-visiblepanel-tab-color") === "#f0f0f0bd" &&
    zone(DOCK, "--dv-inactivegroup-visiblepanel-tab-color") === "#f0f0f0bd",
);
check(
  "the Workshop's active group reads #F0F0F0 whatever Dockview thinks is active",
  zone(".ws-dock [data-ws-active-group]", "--dv-activegroup-visiblepanel-tab-color") === "#f0f0f0" &&
    zone(".ws-dock [data-ws-active-group]", "--dv-inactivegroup-visiblepanel-tab-color") === "#f0f0f0",
);
check("separators are #F0F0F013", zone(DOCK, "--dv-separator-border") === "#f0f0f013");
check("the drop overlay is #F0F0F011", zone(DOCK, "--dv-drag-over-background-color") === "#f0f0f011");
check("the tab drop line is #F0F0F0", zone(DOCK, "--dv-drag-over-border-color") === "#f0f0f0");
check(
  "a hovered sash lights #F0F0F026",
  zone(DOCK, "--dv-active-sash-color") === "color-mix(in srgb, #f0f0f0 15%, transparent)",
);
check("a sash lights after 300ms", zone(DOCK, "--dv-active-sash-transition-delay") === "300ms");
check("a sash fades in over 100ms", zone(DOCK, "--dv-active-sash-transition-duration") === "100ms");
check("tab close hovers are #5A5D5E50", zone(DOCK, "--dv-icon-hover-background-color") === "#5a5d5e50");

// --- The editor tabs ------------------------------------------------------------------------

const MAIN = '[data-ws-zone="main"]';
check("a tab is at least 120px wide", zone(`${MAIN} .dv-tab`, "min-inline-size") === "120px");
check("a tab has a 1px #F0F0F013 right border", zone(`${MAIN} .dv-tab`, "border-inline-end") === "1px solid #f0f0f013");
check(
  "the strip has a 1px #F0F0F013 underline",
  (zone(`${MAIN} .dv-tabs-and-actions-container`, "box-shadow") ?? "").includes("#f0f0f013"),
);
check(
  "the active tab's underline is #181818, so it merges into the editor",
  (zone(`${MAIN} .dv-tab.dv-active-tab`, "box-shadow") ?? "").includes("#181818"),
);
check(
  "Dockview's own tab dividers are dropped for the right border",
  zone(`.ws-dock ${MAIN} .dv-tabs-container .dv-tab:not(:first-child)::before`, "display") === "none",
);
check("a tab has a 10px left padding", zone(`${MAIN} .dv-tab .dv-default-tab`, "padding-inline-start") === "10px");
check(
  "a tab with no close button has a 10px right padding",
  zone(`${MAIN} .dv-tab .dv-default-tab.ws-tab--no-close`, "padding-inline-end") === "10px",
);
check("tab text is 13px", zone(DOCK, "--dv-tab-font-size") === "13px");
check(
  "the hover wash is #2A2A2AB3 by default",
  zone(DOCK, "--ws-tab-hover-wash") === "#2a2a2ab3",
);
check(
  "the active group and the side zones take no hover wash",
  zone(".ws-dock [data-ws-active-group]", "--ws-tab-hover-wash") === "transparent" &&
    zone('.ws-dock [data-ws-zone="left"]', "--ws-tab-hover-wash") === "transparent" &&
    zone('.ws-dock [data-ws-zone="right"]', "--ws-tab-hover-wash") === "transparent",
);
check(
  "a hovered editor tab draws the wash",
  (zone(`${MAIN} .dv-tab:hover`, "background-image") ?? "").includes("--ws-tab-hover-wash"),
);

// --- The close button -----------------------------------------------------------------------

const ACTION = `${MAIN} .dv-tab .dv-default-tab .dv-default-tab-action`;
check("the close button is 20px wide and tall", zone(ACTION, "inline-size") === "20px" && zone(ACTION, "block-size") === "20px");
check("the close button sits in a 28px slot: 20px and 4px each side", zone(ACTION, "margin-inline") === "4px");
check("the close button has a 5px radius", zone(ACTION, "border-radius") === "5px");
check("the close button is hidden by default", zone(ACTION, "opacity") === "0");
check("the close button hovers #5A5D5E50", zone(`${ACTION}:hover`, "background-color") === "#5a5d5e50");
check(
  "the close button is fully visible on the active tab of the active group",
  zone(`[data-ws-active-group] .dv-tab.dv-active-tab .dv-default-tab .dv-default-tab-action`, "opacity") === "1",
);
check(
  "the close button is fully visible on a hovered tab of the active group",
  zone(`[data-ws-active-group] .dv-tab:hover .dv-default-tab .dv-default-tab-action`, "opacity") === "1",
);
check(
  "the close button is half visible on the active tab of another group",
  zone(`${MAIN}:not([data-ws-active-group]) .dv-tab.dv-active-tab .dv-default-tab .dv-default-tab-action`, "opacity") === "0.5",
);
check(
  "the close button is half visible on a hovered tab of another group",
  zone(`${MAIN}:not([data-ws-active-group]) .dv-tab:hover .dv-default-tab .dv-default-tab-action`, "opacity") === "0.5",
);

// --- The dirty dot ----------------------------------------------------------------------------

const DOT = `${MAIN} .dv-default-tab.ws-tab--dirty::after`;
check("a dirty tab draws an 8px dot", zone(DOT, "inline-size") === "8px" && zone(DOT, "block-size") === "8px");
check("the dot is round", zone(DOT, "border-radius") === "9999px");
check("the dot sits where the close button's centre is: 10px from the edge", zone(DOT, "inset-inline-end") === "10px");
check("hovering the tab takes the dot away", zone(`${MAIN} .dv-default-tab.ws-tab--dirty:hover::after`, "display") === "none");
check(
  "the close button stays out of sight on a dirty tab until the tab is hovered",
  zone(`${MAIN} .dv-tab .dv-default-tab.ws-tab--dirty:not(:hover) .dv-default-tab-action`, "visibility") === "hidden",
);

// --- The empty editor group ---------------------------------------------------------------------

const EMPTY = `${MAIN}[data-ws-empty]`;
check("an empty editor group is #141414", zone(EMPTY, "--dv-group-view-background-color") === "#141414");
check("an empty editor group hides its tab strip", zone(`${EMPTY} .dv-tabs-and-actions-container`, "display") === "none");

// --- The left zone's header ---------------------------------------------------------------------

const LEFT = '[data-ws-zone="left"]';
check("the left header's tabs are centered", zone(`${LEFT} .dv-tabs-container`, "justify-content") === "center");
check("the left header's pills are 2px apart", zone(`${LEFT} .dv-tabs-container`, "gap") === "2px");
check("a pill is 22px tall", zone(`${LEFT} .dv-tab .dv-default-tab`, "block-size") === "22px");
check("a pill has a 4px radius", zone(`${LEFT} .dv-tab .dv-default-tab`, "border-radius") === "4px");
check(
  "a pill reads 12px at weight 500",
  zone(`${LEFT} .dv-tab .dv-default-tab`, "font-size") === "12px" &&
    zone(`${LEFT} .dv-tab .dv-default-tab`, "font-weight") === "500",
);
check("a pill's text is #F0F0F0BD", zone(`${LEFT} .dv-tab .dv-default-tab`, "color") === "#f0f0f0bd");
check("a hovered pill is #F0F0F011", zone(`${LEFT} .dv-tab .dv-default-tab:hover`, "background-color") === "#f0f0f011");

// --- The tree ----------------------------------------------------------------------------------------

check("the tree has no padding", treeValue(".ws-workshop-tree", "padding") === "0");
check(
  "the tree reserves no scrollbar gutter",
  [undefined, "auto"].includes(valueIn(tree, ".ws-workshop-tree", "scrollbar-gutter")),
);
check("tree text is #F0F0F0BD", treeValue(".ws-workshop-tree__row", "color") === "#f0f0f0bd");
check("a row spans the tree", treeValue(".ws-workshop-tree__row", "inline-size") === "100%");
check("a row is 22px tall", treeValue(".ws-workshop-tree__row", "block-size") === "22px");
check("a row has a 4px inset", treeValue(".ws-workshop-tree__row", "padding-inline-start") === "4px");
check("a hovered row is #F0F0F011", treeValue(".ws-workshop-tree__row:hover", "background-color") === "#f0f0f011");
check("a focused row is #F0F0F01E", treeValue(".ws-workshop-tree__row:focus-visible", "background-color") === "#f0f0f01e");
check(
  "a focused row draws the 1px #F0F0F026 outline inside itself",
  treeValue(".ws-workshop-tree__row:focus-visible", "outline") === "1px solid color-mix(in srgb, #f0f0f0 15%, transparent)" &&
    treeValue(".ws-workshop-tree__row:focus-visible", "outline-offset") === "-1px",
);
check(
  "the twistie box is 8px per depth plus 16px plus 6px",
  treeValue(".ws-workshop-tree__twistie", "inline-size") === "calc(var(--ws-tree-depth) * 8px + 16px + 6px)",
);
check(
  "the twistie's chevron indents 8px per depth",
  treeValue(".ws-workshop-tree__twistie", "padding-inline-start") === "calc(var(--ws-tree-depth) * 8px)",
);
check(
  "a collapsed folder's chevron is turned -90deg",
  treeValue('.ws-workshop-tree__row[aria-expanded="false"] .ws-workshop-tree__twistie svg', "transform") ===
    "rotate(-90deg)",
);
check(
  "an indent guide is a 1px line at 20px plus 8px per level",
  treeValue(".ws-workshop-tree__children::before", "inset-inline-start") ===
    "calc(20px + (var(--ws-tree-depth) - 1) * 8px)" &&
    treeValue(".ws-workshop-tree__children::before", "inline-size") === "1px",
);
check(
  "guides are #F0F0F013 while the tree is hovered",
  treeValue(".ws-workshop-tree:hover", "--ws-tree-guide") === "#f0f0f013",
);
check(
  "the list holding the focused row lights its own guide #F0F0F030",
  treeValue(".ws-workshop-tree__children--focused::before", "background-color") === "#f0f0f030",
);
// A custom property inherits, so a lit color carried on --ws-tree-guide by the
// focused list would reach every expanded list nested inside it (workshop-zones
// pins that only the list holding the focused row has the focused class).
check(
  "the lit color is not carried by an inherited custom property",
  tree
    .filter((rule) => declaration(rule.body, "--ws-tree-guide") !== undefined)
    .flatMap((rule) => rule.selectors)
    .join("|") === ".ws-workshop-tree:hover",
);
check(
  "the focused guide's rule follows the base guide's, so it wins at equal specificity",
  tree.findIndex((rule) => rule.selectors.includes(".ws-workshop-tree__children--focused::before")) >
    tree.findIndex((rule) => rule.selectors.includes(".ws-workshop-tree__children::before")),
);
check("the section header sits on #141414", treeValue(".ws-workshop-tree__header", "background-color") === "#141414");
check(
  "the section header has no bottom border",
  [undefined, "0", "none"].includes(valueIn(tree, ".ws-workshop-tree__header", "border-bottom")),
);
check("the section title is #F0F0F05C", treeValue(".ws-workshop-tree__section-title", "color") === "#f0f0f05c");
check("the section chevron is 16px", treeValue(".ws-workshop-tree__section-chevron", "inline-size") === "16px");
check("header actions are hidden until the header is hovered", treeValue(".ws-workshop-tree__actions", "opacity") === "0");
check(
  "hovering or focusing within the header shows its actions",
  treeValue(".ws-workshop-tree__header:hover .ws-workshop-tree__actions", "opacity") === "1" &&
    treeValue(".ws-workshop-tree__header:focus-within .ws-workshop-tree__actions", "opacity") === "1",
);
check(
  "a header action is a 20px button with a 5px radius",
  treeValue(".ws-workshop-tree__add", "inline-size") === "20px" &&
    treeValue(".ws-workshop-tree__add", "block-size") === "20px" &&
    treeValue(".ws-workshop-tree__add", "border-radius") === "5px",
);
check(
  "the drag pill is #F0F0F01E with a 10px radius and 12px text",
  treeValue(".ws-workshop-tree__drag-pill", "background-color") === "#f0f0f01e" &&
    treeValue(".ws-workshop-tree__drag-pill", "border-radius") === "10px" &&
    treeValue(".ws-workshop-tree__drag-pill", "font-size") === "12px",
);

if (failures.length > 0) {
  console.error(`dock-css: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("dock-css: all assertions passed");
