// The bundled app mounts the whole workbench into dist/index.html: dockview
// initializes inside #dock with the Workshop tree and one chat panel, ChatUI
// renders the murm structure in its empty-chat state, the voice plugin
// inserts the mic button without a stray status element, and the status bar
// boots as a <footer> landmark reading "Ready" with its progress bar hidden
// and its activity LED present. Guards the DOM contract between index.html
// and the vendored murm-ui (its components throw when a required class is
// missing). Run: node test/workbench-mount.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("the bundled app mounts the whole workbench", async (ctx) => {
  const { document, failures, mic, history, input, send, statusBar, statusText, statusSlot, progressEl, ledEl } = ctx;

  const dock = document.querySelector("#dock");
  const app = document.querySelector("#dock .mur-app");
  if (!dock) failures.push("#dock missing");
  if (dock && !dock.querySelector(".dv-dockview")) {
    failures.push("dockview did not initialize inside #dock");
  }
  if (!document.querySelector("#dock .dv-groupview")) {
    failures.push("dockview rendered no group for the chat panel");
  }
  if (!document.querySelector("#dock .workshop-tree")) {
    failures.push("the Workshop tree panel did not mount in the dock");
  }
  if (!app) failures.push(".mur-app missing inside the dock");
  if (!history) failures.push(".mur-chat-history missing");
  if (!input) failures.push(".mur-chat-input missing");
  if (!send) failures.push(".mur-send-btn missing");
  if (!mic) failures.push("voice plugin did not insert the mic button");
  if (document.querySelector(".voice-status")) {
    failures.push("a .voice-status element exists after the voice plugin mounted");
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
  if (app && !app.classList.contains("mur-chat-empty")) {
    failures.push("fresh mount is not in the empty-chat state");
  }
});
