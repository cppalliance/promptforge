// The bundled app mounts the whole workbench into dist/index.html: dockview
// initializes inside #dock with the Workshop tree and the agent-session
// panel (its menu visible, its session view hidden until a session is
// acknowledged, its input pinned closed), and the status bar boots as a
// <footer> landmark reading "Ready" with its progress bar hidden and its
// activity LED present. Guards the DOM contract between index.html and
// the panel factories. Run: node test/workbench-mount.mjs (after `npm run
// build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("the bundled app mounts the whole workbench", async (ctx) => {
  const { document, failures, agentPanel, statusBar, statusText, statusSlot, progressEl, ledEl } = ctx;

  const dock = document.querySelector("#dock");
  if (!dock) failures.push("#dock missing");
  if (dock && !dock.querySelector(".dv-dockview")) {
    failures.push("dockview did not initialize inside #dock");
  }
  if (!document.querySelector("#dock .dv-groupview")) {
    failures.push("dockview rendered no panel groups");
  }
  if (!document.querySelector("#dock .workshop-tree")) {
    failures.push("the Workshop tree panel did not mount in the dock");
  }
  if (!agentPanel) {
    failures.push("the agent-session panel did not mount in the dock");
  } else {
    const menu = agentPanel.querySelector(".agent-menu");
    const view = agentPanel.querySelector(".agent-session");
    if (!menu) failures.push("the agent menu did not mount inside the agent panel");
    if (menu?.hidden) failures.push("the agent menu must show before a session is acknowledged");
    if (!view) failures.push("the agent-session view did not mount inside the agent panel");
    if (view && !view.hidden) {
      failures.push("the session view must stay hidden until a session is acknowledged");
    }
    const input = view?.querySelector(".prompt-input__editor");
    if (!input) {
      failures.push("the agent-session input did not mount");
    } else if (input.getAttribute("contenteditable") !== "false") {
      failures.push("the agent input must start disabled with no pending wait");
    }
  }
  if (!statusBar) failures.push("status bar placeholder missing");
  if (statusBar && statusBar.tagName !== "FOOTER") {
    failures.push("the status bar is not a <footer> landmark");
  }
  if (!statusText) {
    failures.push("status bar text element missing");
  } else if (statusText.textContent !== "Ready") {
    failures.push(`status bar placeholder text is "${statusText.textContent}", expected "Ready"`);
  }
  if (!statusSlot) failures.push("status bar slot missing");
  if (!progressEl) {
    failures.push("status bar progress element missing");
  } else if (!progressEl.hidden) {
    failures.push("progress bar must start hidden");
  }
  if (!ledEl) failures.push("status bar activity LED missing");
});
