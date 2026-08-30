// Composition root for the gateway config SPA. It mounts the shell
// skeleton - the tab bar and an empty content region - so the styled
// chrome is demonstrable; the router, profile switcher, and views land
// in later commits and take over the <main> body.

import "./styles/base.css";
import "./styles/controls.css";
import "./styles/layout.css";

/** The top-level destinations, in tab order; each maps to a hash route. */
const TABS: ReadonlyArray<readonly [label: string, hash: string]> = [
  ["Models", "#/models"],
  ["Discover", "#/discover"],
  ["Downloads", "#/downloads"],
  ["Profiles", "#/profiles"],
  ["Secrets", "#/secrets"],
  ["Settings", "#/settings"],
];

/** Mounts the shell skeleton into the given root element. */
export function mountShell(root: HTMLElement): void {
  const main = document.createElement("main");
  main.id = "main";
  main.className = "shell";
  // Focusable only programmatically, as the skip link's landing spot.
  main.tabIndex = -1;

  const skip = document.createElement("a");
  skip.className = "skip-link";
  skip.href = "#main";
  skip.textContent = "Skip to main content";
  // The hash belongs to the router (step 15); focus the region
  // directly so the skip jump never rewrites the route fragment.
  skip.addEventListener("click", (event) => {
    event.preventDefault();
    main.focus();
  });

  const header = document.createElement("header");
  header.className = "tab-bar";

  const nav = document.createElement("nav");
  nav.setAttribute("aria-label", "Primary");
  nav.className = "tab-list";
  for (const [label, hash] of TABS) {
    const tab = document.createElement("a");
    tab.className = "tab";
    tab.href = hash;
    tab.textContent = label;
    if (hash === "#/models") {
      tab.setAttribute("aria-current", "page");
    }
    nav.append(tab);
  }
  header.append(nav);

  const title = document.createElement("h1");
  title.className = "shell-title";
  title.textContent = "PromptForge Gateway Config";
  main.append(title);

  root.append(skip, header, main);
}

const app = document.querySelector<HTMLElement>("#app");
if (app) {
  mountShell(app);
}
