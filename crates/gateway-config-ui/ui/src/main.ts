// Composition root for the gateway config SPA. Boot detects the mode:
// the workshop panel (`?mode=panel`) mounts the shell without medallion
// or key prompt - its API access rides the postMessage bridge to the
// workshop, which forwards calls with the bearer key attached, so the
// key never enters this frame - while standalone mounts the key prompt
// first (when no key is stored) and then the live shell: tab bar,
// profile switcher, hash router, and the progress subscription. In
// panel mode the workshop owns all progress display, so the shell never
// subscribes to the progress stream and instead announces apply and
// revert actions to the parent.

import "./styles/base.css";
import "./styles/controls.css";
import "./styles/layout.css";

import { createApplyOverlay } from "./components/apply-overlay";
import { confirmDialog } from "./components/confirm-modal";
import { mountKeyPrompt } from "./components/key-prompt";
import { createProfileSwitcher } from "./components/profile-switcher";
import { openReviewDiff } from "./components/review-diff";
import { createTabBar } from "./components/tab-bar";
import { createToastStack } from "./components/toast";
import { startRouter } from "./router";
import { ConfigStore } from "./services/config-store";
import { GatewayApi } from "./services/gateway-api";
import type { FetchLike } from "./services/gateway-api";
import { HfApi } from "./services/hf-api";
import { PanelBridge, parseBridgeOrigin, type BridgeWindow } from "./services/panel-bridge";
import { createDiscoverView } from "./views/discover-view";
import { createModelsView } from "./views/models-view";
import { createProfilesView } from "./views/profiles-view";
import { createSecretsView } from "./views/secrets-view";
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
  /** Panel-mode outgoing-post seam; production posts to the parent window. */
  bridgePost?: (message: unknown) => void;
  /** Panel-mode bridge reply deadline override, for tests. */
  bridgeTimeoutMs?: number;
}

/** Boots the SPA into `root`. */
export function boot(root: HTMLElement, options: BootOptions = {}): void {
  const win = options.win ?? (window as unknown as BootWindow);
  const fetchFn = options.fetchFn ?? ((input, init) => fetch(input, init));
  const params = new URLSearchParams(win.location.search);

  if (params.get("mode") === "panel") {
    mountPanelMode(root, win, params.get("bridge"), options);
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
    dispose = mountLiveShell(root, win, api, null);
  };
  // Any 401 clears the stored key and returns to the prompt.
  api.onUnauthorized = showPrompt;
  if (api.hasKey()) {
    showShell();
  } else {
    // The `/auth` handoff lands here with an HttpOnly cookie and no
    // stored key: probe once, mounting the shell when the cookie carries
    // auth and the key prompt otherwise.
    void api.hasAmbientAuth().then((authenticated) => {
      if (authenticated) {
        showShell();
      } else {
        showPrompt();
      }
    });
  }
}

/**
 * Boots panel mode. Without a usable `bridge` origin parameter the
 * shell stays inert (the bridge-pending banner, no network calls at
 * all). With one, the bridge announces itself to the pinned workshop
 * origin, waits for the context message, and then mounts the live
 * shell whose transport is the bridge - no sessionStorage key and no
 * direct gateway fetch exist in this frame.
 */
function mountPanelMode(
  root: HTMLElement,
  win: BootWindow,
  bridgeParam: string | null,
  options: BootOptions,
): void {
  const origin = parseBridgeOrigin(bridgeParam);
  if (origin === null) {
    mountPanelPending(root, win);
    return;
  }
  const bridge = new PanelBridge({
    win: win as unknown as BridgeWindow,
    origin,
    post: options.bridgePost,
    timeoutMs: options.bridgeTimeoutMs,
  });
  // Whether the iframe URL itself carried a route, read before the
  // pending shell's router normalizes an empty hash to #/local: an
  // explicit hash outranks the workshop's initial-route context.
  const hadInitialHash = win.location.hash !== "";
  const disposePending = mountPanelPending(root, win);
  let mounted = false;
  bridge.onContext = (context) => {
    // Theme context: the shell's CSS keys off the attribute, and any
    // later context message keeps it fresh without remounting.
    root.setAttribute("data-theme", context.theme);
    if (mounted) {
      return;
    }
    mounted = true;
    disposePending();
    if (!hadInitialHash && context.route.startsWith("#/")) {
      win.location.hash = context.route;
    }
    const api = new GatewayApi({ fetchFn: bridge.fetchLike, storage: memoryStorage(), base: "" });
    mountLiveShell(root, win, api, bridge);
  };
  bridge.start();
}

/**
 * An in-memory Storage stand-in for panel mode: the frame never holds a
 * bearer key, so nothing must ever reach sessionStorage.
 */
function memoryStorage(): Storage {
  const map = new Map<string, string>();
  return {
    get length() {
      return map.size;
    },
    clear: () => map.clear(),
    getItem: (key: string) => map.get(key) ?? null,
    key: (index: number) => [...map.keys()][index] ?? null,
    removeItem: (key: string) => {
      map.delete(key);
    },
    setItem: (key: string, value: string) => {
      map.set(key, value);
    },
  };
}

/**
 * Mounts the live shell and starts its data flows, in either mode:
 * standalone (`bridge` null - medallion, progress subscription) or
 * workshop panel (`bridge` set - no medallion, no progress subscription
 * because the workshop owns progress display, and apply/revert are
 * announced to the parent). Returns the teardown that stops the router
 * and subscriptions.
 */
function mountLiveShell(
  root: HTMLElement,
  win: BootWindow,
  api: GatewayApi,
  bridge: PanelBridge | null,
): () => void {
  const toasts = createToastStack();
  const overlay = createApplyOverlay(root);
  const store = new ConfigStore(api);
  const switcher = createProfileSwitcher({ store, toasts });
  let applying = false;
  let disposed = false;
  let observedConfigGeneration: string | null = null;
  let restartTimer: ReturnType<typeof setTimeout> | null = null;

  const restartBanner = document.createElement("div");
  restartBanner.className = "banner banner-warning banner-restart";
  restartBanner.hidden = true;
  restartBanner.textContent = "Restart the gateway to apply these changes.";

  const watchForRestart = (): void => {
    if (restartTimer !== null) {
      clearTimeout(restartTimer);
    }
    const poll = async (): Promise<void> => {
      if (disposed) {
        return;
      }
      try {
        const status = await api.getStatus();
        if (
          observedConfigGeneration !== null &&
          status.config_generation !== "" &&
          status.config_generation !== observedConfigGeneration
        ) {
          observedConfigGeneration = status.config_generation;
          restartBanner.hidden = true;
          restartTimer = null;
          return;
        }
      } catch {
        // A restart briefly drops the listener; the next poll retries.
      }
      if (disposed) {
        return;
      }
      restartTimer = setTimeout(() => void poll(), 1_000);
    };
    void poll();
  };

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
        watchForRestart();
      }
      toasts.show(
        outcome.restart_required
          ? "Configuration applied - restart the gateway to finish"
          : "Configuration applied",
        "success",
      );
      bridge?.notifyAction("apply");
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
      bridge?.notifyAction("revert");
    } catch (error) {
      toasts.show(error instanceof Error ? error.message : "The revert failed", "error");
    }
  };

  const tabBar = createTabBar({
    showMedallion: bridge === null,
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
    const review = document.createElement("button");
    review.type = "button";
    review.className = "button button-xs button-outline banner-review";
    review.textContent = "Review";
    review.addEventListener("click", () => openReviewDiff(root, store.pendingDiff()));
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
    banner.replaceChildren(text, review, apply, revert);
  };
  const unsubscribe = store.subscribe(renderPendingState);

  const bannerBox = document.createElement("div");
  bannerBox.className = "banner-stack";
  bannerBox.append(restartBanner, banner);
  const main = mountChrome(root, tabBar.element, [toasts.element], bannerBox);

  api.onHealth = (ok) => tabBar.setConnected(ok);
  const localView = createModelsView({ store, api, toasts, scope: "local" });
  const remoteView = createModelsView({ store, api, toasts, scope: "remote" });
  const settingsView = createSettingsView({ store, api, toasts });
  const discoverView = createDiscoverView({
    api,
    hf: new HfApi(api),
    store,
    toasts,
  });
  const secretsView = createSecretsView({ store, api, toasts });
  const profilesView = createProfilesView({
    store,
    toasts,
  });
  const stopRouter = startRouter({
    win,
    main,
    onRoute: (view) => tabBar.setActiveView(view),
    views: {
      local: (target, match) => localView.mount(target, match.detail),
      remote: (target, match) => remoteView.mount(target, match.detail),
      discover: (target) => discoverView.mount(target),
      profiles: (target) => profilesView.mount(target),
      secrets: (target) => secretsView.mount(target),
      settings: (target, match) => settingsView.mount(target, match.detail),
    },
  });

  void api
    .getStatus()
    .then((status) => {
      observedConfigGeneration = status.config_generation;
      switcher.setActiveProfile(status.profile);
    })
    .catch(() => {
      // The dot already went red via onHealth; a 401 already routed to
      // the key prompt via onUnauthorized.
    });
  void store.load();
  // The live progress stream: while an apply is in flight, stage-shaped
  // events feed the overlay. Subscribing at boot keeps the shell an
  // independent subscriber whether or not the workshop is connected.
  // Panel mode never subscribes: the workshop already consumes the same
  // stream and owns all progress display.
  const stopProgress =
    bridge === null
      ? api.subscribeProgress((event) => {
          if (!applying || event === null || typeof event !== "object") {
            return;
          }
          const stage = (event as Record<string, unknown>)["stage"];
          if (typeof stage === "string") {
            overlay.beginStage(stage);
          }
        })
      : () => undefined;
  return () => {
    disposed = true;
    stopRouter();
    stopProgress();
    unsubscribe();
    if (restartTimer !== null) {
      clearTimeout(restartTimer);
    }
  };
}

/**
 * Mounts the inert panel-mode shell: the same chrome minus the
 * medallion and key prompt, with no network calls at all. It shows
 * until the workshop's context message arrives (and for good when the
 * iframe URL carries no usable bridge origin); the profile switcher is
 * an inert placeholder and a banner says so. Returns the teardown that
 * stops its router, so the live shell can replace it cleanly.
 */
function mountPanelPending(root: HTMLElement, win: BootWindow): () => void {
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
  return startRouter({ win, main, onRoute: (view) => tabBar.setActiveView(view) });
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
