// The top tab bar [Adapted: Unsloth]: medallion left (standalone only),
// the profile switcher, six icon+label tabs whose active state is the
// accent underline, and the right cluster holding the connection dot
// [Adapted: llama-swap] plus the container the Apply/Revert pair mounts
// into when the write path lands.

import {
  Cpu,
  Folder,
  Globe,
  Key,
  Search,
  Settings,
  createElement as lucideElement,
} from "lucide";
import type { IconNode } from "lucide";

import type { ViewId } from "../router";
import { programIcon } from "./program-icon";

// The crate version, substituted by the esbuild define in build.mjs; a
// bundle built without the define shows the "dev" fallback instead of
// breaking on a free identifier.
declare const __APP_VERSION__: string | undefined;
const APP_VERSION = typeof __APP_VERSION__ === "string" ? __APP_VERSION__ : "dev";

/** One tab: destination view, label, lucide icon, and hash target. */
const TABS: ReadonlyArray<readonly [view: ViewId, label: string, icon: IconNode, href: string]> = [
  ["settings", "Settings", Settings, "#/settings"],
  ["discover", "Discover", Search, "#/discover"],
  ["local", "Local", Cpu, "#/local"],
  ["remote", "Remote", Globe, "#/remote"],
  ["profiles", "Profiles", Folder, "#/profiles"],
  ["secrets", "Secrets", Key, "#/secrets"],
];

/** Construction options for the tab bar. */
export interface TabBarOptions {
  /** Standalone mode shows the medallion; the workshop panel hides it. */
  showMedallion: boolean;
  /** The profile switcher element (or its panel-mode placeholder). */
  switcher: HTMLElement;
  /** Fired when the Apply (N) button is pressed. */
  onApply?: () => void;
  /** Fired when the Revert All button is pressed. */
  onRevertAll?: () => void;
}

/** The mounted tab bar and its live-update handles. */
export interface TabBar {
  /** The `<header class="tab-bar">` element. */
  element: HTMLElement;
  /** Moves `aria-current` (and the accent underline) to `view`. */
  setActiveView(view: ViewId | null): void;
  /** Recolors the connection dot from the latest API outcome. */
  setConnected(ok: boolean): void;
  /**
   * Shows Apply (N) + Revert All when `count` is positive, hides the
   * pair at zero. `count` is the pending-file count from the dirty
   * report.
   */
  setPendingCount(count: number): void;
}

/** Builds the tab bar. */
export function createTabBar(options: TabBarOptions): TabBar {
  const element = document.createElement("header");
  element.className = "tab-bar";

  if (options.showMedallion) {
    const medallion = programIcon(24, "PromptForge");
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
  const version = document.createElement("span");
  version.className = "tab-version";
  version.textContent = `v${APP_VERSION}`;
  const dot = document.createElement("span");
  dot.className = "status-dot";
  const dotText = document.createElement("span");
  dotText.className = "visually-hidden";
  dotText.textContent = "Gateway status unknown";
  dot.append(dotText);
  // The Apply/Revert pair [INVENTED] renders in here whenever the dirty
  // report says shadow files exist.
  const pending = document.createElement("div");
  pending.className = "apply-actions";
  actions.append(version, dot, pending);
  element.append(actions);

  return {
    element,
    setPendingCount(count: number): void {
      if (count <= 0) {
        pending.replaceChildren();
        return;
      }
      const apply = document.createElement("button");
      apply.type = "button";
      apply.className = "button button-sm button-primary apply-button";
      apply.textContent = `Apply (${count})`;
      apply.addEventListener("click", () => options.onApply?.());
      const revert = document.createElement("button");
      revert.type = "button";
      revert.className = "button button-sm button-outline revert-button";
      revert.textContent = "Revert All";
      revert.addEventListener("click", () => options.onRevertAll?.());
      pending.replaceChildren(apply, revert);
    },
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
