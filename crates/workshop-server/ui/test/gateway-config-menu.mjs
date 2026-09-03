// Bundle-level test for the Gateway Config menu path: the Window menu
// lists "Gateway Config" next to Workshop Panel, activating it opens a
// dockview panel hosting the config SPA's iframe (proxied same-origin
// at /gateway/config/, panel mode and the workshop's own origin in the
// query), and a second activation focuses the existing panel instead of
// opening another.
// Run: node test/gateway-config-menu.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("the Gateway Config menu item opens the panel", async ({ window, document, sleep, failures }) => {
  const windowButton = document.querySelector('[data-menu="window"]');
  if (!windowButton) {
    failures.push("the Window menu button is missing");
    return;
  }
  windowButton.click();
  const popover = windowButton.nextElementSibling;
  const items = [...popover.querySelectorAll(".window-titlebar__item")];
  const labels = items.map(
    (item) => item.querySelector(".window-titlebar__item-label")?.textContent,
  );
  const configItem = items[labels.indexOf("Gateway Config")];
  if (!configItem) {
    failures.push(`the Window menu lists no Gateway Config item (found: ${labels.join(",")})`);
    return;
  }
  if (labels.indexOf("Gateway Config") !== labels.indexOf("Workshop Panel") + 1) {
    failures.push("Gateway Config does not sit next to Workshop Panel");
  }

  configItem.click();
  // The panel mounts the iframe synchronously (no async origin probe).
  await sleep(50);
  const iframe = document.querySelector(".gateway-config-panel__frame");
  if (!iframe) {
    failures.push("activating Gateway Config never mounted the panel iframe");
    return;
  }
  const expectedSrc = `/gateway/config/?mode=panel&bridge=${encodeURIComponent(
    window.location.origin,
  )}`;
  if (iframe.getAttribute("src") !== expectedSrc) {
    failures.push(`the iframe src is "${iframe.getAttribute("src")}", expected "${expectedSrc}"`);
  }
  if (iframe.getAttribute("sandbox") !== "allow-scripts allow-same-origin") {
    failures.push("the iframe sandbox is not allow-scripts allow-same-origin");
  }
  const tabs = [...document.querySelectorAll(".dv-default-tab-content")].map(
    (tab) => tab.textContent,
  );
  if (!tabs.includes("Gateway Config")) {
    failures.push(`no dockview tab is titled Gateway Config (found: ${tabs.join(",")})`);
  }

  // A second activation focuses the existing panel; it never duplicates.
  windowButton.click();
  const again = [...windowButton.nextElementSibling.querySelectorAll(".window-titlebar__item")].find(
    (item) => item.querySelector(".window-titlebar__item-label")?.textContent === "Gateway Config",
  );
  again?.click();
  await sleep(100);
  const frames = document.querySelectorAll(".gateway-config-panel__frame");
  if (frames.length !== 1) {
    failures.push(`reopening Gateway Config left ${frames.length} iframes, expected 1`);
  }

  // Close the panel through its tab, so the run leaves no undisposed
  // panel behind (the leak check would flag it otherwise).
  const configTab = [...document.querySelectorAll(".dv-default-tab")].find(
    (tab) => tab.querySelector(".dv-default-tab-content")?.textContent === "Gateway Config",
  );
  const closeAction = configTab?.querySelector(".dv-default-tab-action");
  if (!closeAction) {
    failures.push("the Gateway Config tab offers no close action");
    return;
  }
  closeAction.dispatchEvent(new window.PointerEvent("pointerdown", { bubbles: true }));
  closeAction.click();
  await sleep(50);
  if (document.querySelector(".gateway-config-panel__frame")) {
    failures.push("closing the tab did not remove the panel");
  }
});
