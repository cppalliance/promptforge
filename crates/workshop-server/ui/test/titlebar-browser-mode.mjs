// Custom window title bar in browser mode: the bar carries the application
// menus, so it must be visible after boot even without the Tauri runtime;
// only the native window-control cluster hides, and the module never calls
// into the Tauri window API - the whole test runs with no
// __TAURI_INTERNALS__ defined, so a passing run proves the menu path needs
// no desktop bridge.
// Covers the <header> landmark, the program icon's attributes, the five
// menus and their popovers (File opens and announces aria-expanded), the
// drag region, and the window-control cluster's buttons and glyphs.
// Run: node test/titlebar-browser-mode.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("the title bar works in browser mode without ipc", async ({ window, document, failures }) => {
  const titlebar = document.querySelector(".window-titlebar");
  if (!titlebar) {
    failures.push("window title bar missing");
  } else {
    if (titlebar.tagName !== "HEADER") {
      failures.push("the window title bar is not a <header> landmark");
    }
    if (window.__TAURI_INTERNALS__ !== undefined) {
      failures.push("this test must run without the Tauri runtime present");
    }
    if (titlebar.hidden) {
      failures.push("the title bar must be visible after boot in browser mode");
    }
    const controlsCluster = titlebar.querySelector(".window-titlebar__controls");
    if (!controlsCluster) {
      failures.push("title bar window-control cluster missing");
    } else if (!controlsCluster.hidden) {
      failures.push("the window-control cluster must stay hidden in browser mode");
    }
    const icon = titlebar.querySelector(".window-titlebar__icon");
    if (!icon) {
      failures.push("title bar program icon missing");
    } else {
      if (icon.getAttribute("src") !== "/icons/promptforge-icon.png") {
        failures.push(`title bar icon src is "${icon.getAttribute("src")}"`);
      }
      if (icon.getAttribute("srcset") !== "/icons/promptforge-icon.png 1x, /icons/promptforge-icon@2x.png 2x") {
        failures.push(`title bar icon srcset is "${icon.getAttribute("srcset")}"; it must name the @2x render`);
      }
      if (icon.getAttribute("alt") !== "") {
        failures.push("the decorative title bar icon must carry an empty alt");
      }
      if (!icon.getAttribute("width") || !icon.getAttribute("height")) {
        failures.push("the title bar icon must carry width and height");
      }
    }
    const menuLabels = [...titlebar.querySelectorAll(".window-titlebar__menu")].map(
      (button) => button.textContent,
    );
    if (menuLabels.join(",") !== "File,Edit,Model,Window,Help") {
      failures.push(`title bar menus are "${menuLabels.join(",")}", expected "File,Edit,Model,Window,Help"`);
    }
    for (const button of titlebar.querySelectorAll(".window-titlebar__menu")) {
      if (button.tagName !== "BUTTON") failures.push("a title bar menu is not a <button>");
    }
    const popovers = titlebar.querySelectorAll(".window-titlebar__popover");
    if (popovers.length !== 5) {
      failures.push(`browser mode built ${popovers.length} menu popovers, expected 5`);
    }
    const fileButton = titlebar.querySelector('[data-menu="file"]');
    if (fileButton) {
      fileButton.click();
      const filePopover = fileButton.nextElementSibling;
      if (!filePopover || !filePopover.classList.contains("window-titlebar__popover") || filePopover.hidden) {
        failures.push("clicking the File menu button does not open its popover in browser mode");
      }
      if (fileButton.getAttribute("aria-expanded") !== "true") {
        failures.push("the open File menu button is not announced expanded");
      }
      fileButton.click();
    }
    if (!titlebar.querySelector(".window-titlebar__drag")) {
      failures.push("title bar drag region missing");
    }
    const controls = [...titlebar.querySelectorAll(".window-titlebar__control")];
    const controlLabels = controls.map((button) => button.getAttribute("aria-label"));
    if (controlLabels.join(",") !== "Minimize,Maximize,Close") {
      failures.push(
        `window controls are "${controlLabels.join(",")}", expected "Minimize,Maximize,Close"`,
      );
    }
    for (const button of controls) {
      if (button.tagName !== "BUTTON") {
        failures.push(`window control "${button.getAttribute("aria-label")}" is not a <button>`);
      }
      if (!button.querySelector("svg")) {
        failures.push(`window control "${button.getAttribute("aria-label")}" has no inline SVG glyph`);
      }
    }
  }
  if ("__TAURI_INTERNALS__" in window) {
    failures.push("browser mode must not require the Tauri internals");
  }
});
