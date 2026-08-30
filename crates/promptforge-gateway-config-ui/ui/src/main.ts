// Composition root for the gateway config SPA. Boot detects the mode:
// the workshop panel (`?mode=panel`) mounts the shell without medallion
// or key prompt - its API access arrives with the postMessage bridge -
// while standalone mounts the key prompt first (when no key is stored)
// and then the live shell: tab bar, profile switcher, hash router, and
// the progress subscription.

import "./styles/base.css";
import "./styles/controls.css";
import "./styles/layout.css";

import { createApplyOverlay } from "./components/apply-overlay";
import { mountKeyPrompt } from "./components/key-prompt";
import { createProfileSwitcher } from "./components/profile-switcher";
import { createTabBar } from "./components/tab-bar";
import { createToastStack } from "./components/toast";
import { startRouter } from "./router";
import { GatewayApi } from "./services/gateway-api";
import type { FetchLike } from "./services/gateway-api";

export { API_KEY_STORAGE_KEY } from "./services/gateway-api";
export { matchRoute } from "./router";

/** The window surface boot needs; tests hand in a jsdom window. */
export interface BootWindow {
  /** Location for mode detection (search) and routing (hash). */
  location: { hash: string; search: string };
  /** Where the bearer key lives for the session. */
  sessionStorage: Storage;
  /** Event registration for `hashchange`. */
  addEventListener(type: string, listener: () => void): void;
  /** Event removal, so a torn-down shell leaves no listener behind. */
  removeEventListener(type: string, listener: () => void): void;
}

/** Injectable boot dependencies; production uses the browser globals. */
export interface BootOptions {
  /** The window; defaults to the global one. */
  win?: BootWindow;
  /** The transport; defaults to the global fetch. */
  fetchFn?: FetchLike;
}

/** Boots the SPA into `root`. */
export function boot(root: HTMLElement, options: BootOptions = {}): void {
  const win = options.win ?? (window as unknown as BootWindow);
  const fetchFn = options.fetchFn ?? ((input, init) => fetch(input, init));
  const panel = new URLSearchParams(win.location.search).get("mode") === "panel";

  if (panel) {
    mountPanelShell(root, win);
    return;
  }

  const api = new GatewayApi({ fetchFn, storage: win.sessionStorage });
  // Each remount (401 -> prompt -> shell) first tears the old screen's
  // router and progress subscription down, so cycles never stack them.
  let dispose: () => void = () => undefined;
  const showPrompt = () => {
    dispose();
    dispose = () => undefined;
    mountKeyPrompt(root, { api, onSuccess: showShell });
  };
  const showShell = () => {
    dispose();
    dispose = mountStandaloneShell(root, win, api);
  };
  // Any 401 clears the stored key and returns to the prompt.
  api.onUnauthorized = showPrompt;
  if (api.hasKey()) {
    showShell();
  } else {
    showPrompt();
  }
}

/**
 * Mounts the full standalone shell and starts its data flows. Returns
 * the teardown that stops the router and the progress subscription.
 */
function mountStandaloneShell(root: HTMLElement, win: BootWindow, api: GatewayApi): () => void {
  const toasts = createToastStack();
  const overlay = createApplyOverlay(root);
  const switcher = createProfileSwitcher({ api, overlay, toasts });
  const tabBar = createTabBar({ showMedallion: true, switcher: switcher.element });
  const main = mountChrome(root, tabBar.element, [toasts.element]);

  api.onHealth = (ok) => tabBar.setConnected(ok);
  const stopRouter = startRouter({ win, main, onRoute: (view) => tabBar.setActiveView(view) });

  void api
    .getStatus()
    .then((status) => switcher.setActiveProfile(status.profile))
    .catch(() => {
      // The dot already went red via onHealth; a 401 already routed to
      // the key prompt via onUnauthorized.
    });
  // The live progress stream: downloads and apply progress consume it
  // once those surfaces exist; subscribing at boot keeps the shell an
  // independent subscriber whether or not the workshop is connected.
  const stopProgress = api.subscribeProgress(() => undefined);
  return () => {
    stopRouter();
    stopProgress();
  };
}

/**
 * Mounts the panel-mode shell: the same chrome minus the medallion and
 * key prompt. Gateway data waits on the workshop's postMessage bridge,
 * so the profile switcher is an inert placeholder and a banner says so.
 */
function mountPanelShell(root: HTMLElement, win: BootWindow): void {
  const placeholder = document.createElement("button");
  placeholder.type = "button";
  placeholder.className = "select select-sm";
  placeholder.disabled = true;
  placeholder.textContent = "Profile";

  const tabBar = createTabBar({ showMedallion: false, switcher: placeholder });

  const note = document.createElement("p");
  note.className = "banner";
  note.textContent = "Workshop bridge pending: gateway data is unavailable in panel mode.";

  const main = mountChrome(root, tabBar.element, [], note);
  startRouter({ win, main, onRoute: (view) => tabBar.setActiveView(view) });
}

/**
 * Mounts the shared chrome - skip link, tab bar header, an optional
 * banner, and the `<main>` region - and returns the main element.
 */
function mountChrome(
  root: HTMLElement,
  header: HTMLElement,
  fixed: HTMLElement[],
  banner?: HTMLElement,
): HTMLElement {
  const main = document.createElement("main");
  main.id = "main";
  main.className = "shell";
  // Focusable only programmatically, as the skip link's landing spot.
  main.tabIndex = -1;

  const skip = document.createElement("a");
  skip.className = "skip-link";
  skip.href = "#main";
  skip.textContent = "Skip to main content";
  // The hash belongs to the router; focus the region directly so the
  // skip jump never rewrites the route fragment.
  skip.addEventListener("click", (event) => {
    event.preventDefault();
    main.focus();
  });

  const parts: HTMLElement[] = [skip, header];
  if (banner) {
    parts.push(banner);
  }
  parts.push(main, ...fixed);
  root.replaceChildren(...parts);
  return main;
}

const app = document.querySelector<HTMLElement>("#app");
if (app) {
  boot(app);
}
