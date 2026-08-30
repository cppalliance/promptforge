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
import { confirmDialog } from "./components/confirm-modal";
import { mountKeyPrompt } from "./components/key-prompt";
import { createProfileSwitcher } from "./components/profile-switcher";
import { createTabBar } from "./components/tab-bar";
import { createToastStack } from "./components/toast";
import { startRouter } from "./router";
import { ConfigStore } from "./services/config-store";
import { DownloadStore } from "./services/download-store";
import { GatewayApi } from "./services/gateway-api";
import type { FetchLike } from "./services/gateway-api";
import { HfApi } from "./services/hf-api";
import { createDiscoverView } from "./views/discover-view";
import { createDownloadsView } from "./views/downloads-view";
import { createModelsView } from "./views/models-view";
import { createProfilesView } from "./views/profiles-view";
import { createSettingsView } from "./views/settings-view";

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
    dispose = mountStandaloneShell(root, win, api, fetchFn);
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
function mountStandaloneShell(
  root: HTMLElement,
  win: BootWindow,
  api: GatewayApi,
  fetchFn: FetchLike,
): () => void {
  const toasts = createToastStack();
  const overlay = createApplyOverlay(root);
  const switcher = createProfileSwitcher({ api, overlay, toasts });
  const store = new ConfigStore(api);
  const downloadStore = new DownloadStore(api);
  let applying = false;

  // Restart banner [INVENTED]: raised when a boot-scoped apply promoted
  // the boot shadow. The gateway cannot hot-reload its bind, so the
  // banner persists for the rest of the session; a fresh load after the
  // restart starts with no pending boot shadow and no banner. (The
  // plan's /health boot-generation detection is simplified away.)
  const restartBanner = document.createElement("div");
  restartBanner.className = "banner banner-warning banner-restart";
  restartBanner.hidden = true;
  restartBanner.textContent = "Restart the gateway to apply these changes.";

  const runApply = async (): Promise<void> => {
    if (applying) {
      return;
    }
    applying = true;
    overlay.open("Applying configuration");
    try {
      const outcome = await store.apply();
      overlay.finish();
      if (outcome.restart_required) {
        restartBanner.hidden = false;
      }
      toasts.show(
        outcome.restart_required
          ? "Configuration applied - restart the gateway to finish"
          : "Configuration applied",
        "success",
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : "The apply failed";
      overlay.fail(message);
      toasts.show(message, "error");
    } finally {
      applying = false;
    }
  };

  const runRevertAll = async (): Promise<void> => {
    const count = store.dirty.pending_files.length;
    const yes = await confirmDialog(root, {
      title: "Revert all pending changes?",
      body: `This discards the pending changes across ${count} file${count === 1 ? "" : "s"} and returns to the running configuration.`,
      confirmLabel: "Revert All",
      danger: true,
    });
    if (!yes) {
      return;
    }
    try {
      await store.revertAll();
      toasts.show("Pending changes reverted", "success");
    } catch (error) {
      toasts.show(error instanceof Error ? error.message : "The revert failed", "error");
    }
  };

  const tabBar = createTabBar({
    showMedallion: true,
    switcher: switcher.element,
    onApply: () => void runApply(),
    onRevertAll: () => void runRevertAll(),
  });

  // Pending-changes banner [INVENTED]: raised only when shadows already
  // exist when the shell loads (a previous session's saves), cleared
  // once they are applied or reverted.
  const banner = document.createElement("div");
  banner.className = "banner banner-pending";
  banner.hidden = true;
  let bannerArmed: boolean | null = null;

  const renderPendingState = (): void => {
    const count = store.dirty.pending_files.length;
    tabBar.setPendingCount(count);
    if (bannerArmed === null && store.loaded && !store.loadError) {
      bannerArmed = store.dirty.dirty;
    }
    if (bannerArmed && count === 0) {
      // The previous session's shadows are gone (applied or reverted);
      // later same-session saves are not "from a previous session".
      bannerArmed = false;
    }
    if (!bannerArmed || count === 0) {
      banner.hidden = true;
      return;
    }
    banner.hidden = false;
    const text = document.createElement("span");
    text.textContent = `You have ${count} pending change${count === 1 ? "" : "s"} from a previous session.`;
    const apply = document.createElement("button");
    apply.type = "button";
    apply.className = "button button-xs button-primary banner-apply";
    apply.textContent = "Apply";
    apply.addEventListener("click", () => void runApply());
    const revert = document.createElement("button");
    revert.type = "button";
    revert.className = "button button-xs button-outline banner-revert";
    revert.textContent = "Revert All";
    revert.addEventListener("click", () => void runRevertAll());
    banner.replaceChildren(text, apply, revert);
  };
  const unsubscribe = store.subscribe(renderPendingState);

  // Top progress strip [Adapted: LocalAI]: a thin lava-gradient bar at
  // the very top of the window while any download is active, fed by
  // the global download store so it survives view navigation. The same
  // subscription drives the Downloads tab's active-count badge.
  const strip = document.createElement("div");
  strip.className = "progress-strip global-progress";
  strip.hidden = true;
  const stripBar = document.createElement("div");
  stripBar.className = "progress-strip-bar";
  strip.append(stripBar);
  const renderStrip = (): void => {
    const active = downloadStore.active();
    strip.hidden = active.length === 0;
    stripBar.style.setProperty("--progress", String(downloadStore.overallFraction()));
    tabBar.setDownloadsBadge(active.length);
  };
  const unsubscribeDownloads = downloadStore.subscribe(renderStrip);

  const bannerBox = document.createElement("div");
  bannerBox.className = "banner-stack";
  bannerBox.append(restartBanner, banner);
  const main = mountChrome(root, tabBar.element, [toasts.element], bannerBox, strip);

  api.onHealth = (ok) => tabBar.setConnected(ok);
  const modelsView = createModelsView({ store, api, toasts });
  const settingsView = createSettingsView({ store, api, toasts });
  const discoverView = createDiscoverView({
    api,
    hf: new HfApi(api),
    downloads: downloadStore,
    toasts,
    fetchFn,
  });
  const downloadsView = createDownloadsView({ api, downloads: downloadStore, toasts });
  const profilesView = createProfilesView({
    store,
    api,
    toasts,
    overlay,
    onSwitched: (name) => {
      switcher.setActiveProfile(name);
      // The active profile changed, so the running/pending views did too.
      void store.load();
    },
  });
  const stopRouter = startRouter({
    win,
    main,
    onRoute: (view) => tabBar.setActiveView(view),
    views: {
      models: (target, match) => modelsView.mount(target, match.detail),
      discover: (target) => discoverView.mount(target),
      downloads: (target) => downloadsView.mount(target),
      profiles: (target, match) => profilesView.mount(target, match.detail),
      settings: (target, match) => settingsView.mount(target, match.detail),
    },
  });

  void api
    .getStatus()
    .then((status) => switcher.setActiveProfile(status.profile))
    .catch(() => {
      // The dot already went red via onHealth; a 401 already routed to
      // the key prompt via onUnauthorized.
    });
  void store.load();
  // The live progress stream: while an apply is in flight, stage-shaped
  // events feed the overlay; downloads consume the same stream once
  // that surface exists. Subscribing at boot keeps the shell an
  // independent subscriber whether or not the workshop is connected.
  const stopProgress = api.subscribeProgress((event) => {
    if (!applying || event === null || typeof event !== "object") {
      return;
    }
    const stage = (event as Record<string, unknown>)["stage"];
    if (typeof stage === "string") {
      overlay.beginStage(stage);
    }
  });
  return () => {
    stopRouter();
    stopProgress();
    unsubscribe();
    unsubscribeDownloads();
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
 * Mounts the shared chrome - skip link, an optional top progress
 * strip, tab bar header, an optional banner, and the `<main>` region -
 * and returns the main element.
 */
function mountChrome(
  root: HTMLElement,
  header: HTMLElement,
  fixed: HTMLElement[],
  banner?: HTMLElement,
  strip?: HTMLElement,
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

  const parts: HTMLElement[] = [skip];
  if (strip) {
    parts.push(strip);
  }
  parts.push(header);
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
