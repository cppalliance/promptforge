// Adds an "All docs" link to the mdBook menu bar. mdBook sets the
// `path_to_root` global on every page to the book root, and the landing
// page sits one folder above that. The link names `index.html` rather
// than the folder so it also opens the page under file://.
(function () {
  "use strict";

  var MENU_SELECTOR = "#menu-bar .right-buttons";

  function addLink() {
    var buttons = document.querySelector(MENU_SELECTOR);
    if (!buttons) {
      console.warn(
        "back-link.js: no element matches " + MENU_SELECTOR +
          ", so the \"All docs\" link was not added"
      );
      return;
    }
    var link = document.createElement("a");
    link.href = path_to_root + "../index.html";
    link.textContent = "All docs";
    link.className = "icon-button";
    link.title = "All PromptForge documentation";
    link.setAttribute("aria-label", "All PromptForge documentation");
    buttons.insertBefore(link, buttons.firstChild);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", addLink);
  } else {
    addLink();
  }
})();
