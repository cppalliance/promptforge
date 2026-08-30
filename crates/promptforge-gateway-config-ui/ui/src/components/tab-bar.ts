// The top tab bar [Adapted: Unsloth]: medallion left (standalone only),
// the profile switcher, six icon+label tabs whose active state is the
// accent underline, and the right cluster holding the connection dot
// [Adapted: llama-swap] plus the container the Apply/Revert pair mounts
// into when the write path lands.

import {
  Download,
  Folder,
  Key,
  Layers,
  Search,
  Settings,
  createElement as lucideElement,
} from "lucide";
import type { IconNode } from "lucide";

import type { ViewId } from "../router";

/** One tab: destination view, label, lucide icon, and hash target. */
const TABS: ReadonlyArray<readonly [view: ViewId, label: string, icon: IconNode, href: string]> = [
  ["models", "Models", Layers, "#/models"],
  ["discover", "Discover", Search, "#/discover"],
  ["downloads", "Downloads", Download, "#/downloads"],
  ["profiles", "Profiles", Folder, "#/profiles"],
  ["secrets", "Secrets", Key, "#/secrets"],
  ["settings", "Settings", Settings, "#/settings"],
];

/** Construction options for the tab bar. */
export interface TabBarOptions {
  /** Standalone mode shows the medallion; the workshop panel hides it. */
  showMedallion: boolean;
  /** The profile switcher element (or its panel-mode placeholder). */
  switcher: HTMLElement;
}

/** The mounted tab bar and its live-update handles. */
export interface TabBar {
  /** The `<header class="tab-bar">` element. */
  element: HTMLElement;
  /** Moves `aria-current` (and the accent underline) to `view`. */
  setActiveView(view: ViewId | null): void;
  /** Recolors the connection dot from the latest API outcome. */
  setConnected(ok: boolean): void;
}

/** Builds the tab bar. */
export function createTabBar(options: TabBarOptions): TabBar {
  const element = document.createElement("header");
  element.className = "tab-bar";

  if (options.showMedallion) {
    const medallion = document.createElement("img");
    medallion.src = "icons/promptforge-icon-1.png";
    medallion.alt = "PromptForge";
    medallion.width = 24;
    medallion.height = 24;
    medallion.className = "tab-medallion";
    element.append(medallion);
  }

  element.append(options.switcher);

  const nav = document.createElement("nav");
  nav.setAttribute("aria-label", "Primary");
  nav.className = "tab-list";
  const tabByView = new Map<ViewId, HTMLAnchorElement>();
  for (const [view, label, icon, href] of TABS) {
    const tab = document.createElement("a");
    tab.className = "tab";
    tab.href = href;
    const svg = lucideElement(icon, { "aria-hidden": "true", width: 16, height: 16 });
    const text = document.createElement("span");
    text.textContent = label;
    tab.append(svg, text);
    nav.append(tab);
    tabByView.set(view, tab);
  }
  element.append(nav);

  const actions = document.createElement("div");
  actions.className = "tab-actions";
  const dot = document.createElement("span");
  dot.className = "status-dot";
  const dotText = document.createElement("span");
  dotText.className = "visually-hidden";
  dotText.textContent = "Gateway status unknown";
  dot.append(dotText);
  // The Apply/Revert pair renders in here once shadow writes exist;
  // until then the container keeps the right cluster's layout stable.
  const pending = document.createElement("div");
  pending.className = "apply-actions";
  actions.append(dot, pending);
  element.append(actions);

  return {
    element,
    setActiveView(view: ViewId | null): void {
      for (const [tabView, tab] of tabByView) {
        if (tabView === view) {
          tab.setAttribute("aria-current", "page");
        } else {
          tab.removeAttribute("aria-current");
        }
      }
    },
    setConnected(ok: boolean): void {
      dot.classList.toggle("is-ok", ok);
      dot.classList.toggle("is-bad", !ok);
      dotText.textContent = ok ? "Gateway reachable" : "Gateway unreachable";
    },
  };
}
