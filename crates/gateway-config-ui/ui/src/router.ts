// Hash router [Adapted: llama.cpp] for both shell modes.

/** The six top-level destinations. */
export type ViewId = "settings" | "discover" | "local" | "remote" | "profiles" | "secrets";

/** A parsed route: the view plus its optional detail segment. */
export interface RouteMatch {
  /** The destination view. */
  view: ViewId;
  /** The model name or settings section, when the route carries one. */
  detail?: string;
}

/** Display titles for the stub views. */
const VIEW_TITLES: Readonly<Record<ViewId, string>> = {
  settings: "Settings",
  discover: "Discover",
  local: "Local",
  remote: "Remote",
  profiles: "Profiles",
  secrets: "Secrets",
};

/**
 * Parses a location hash into a route, or null for an unknown hash.
 * A bare `#/settings` normalizes to the first section, System.
 */
export function matchRoute(hash: string): RouteMatch | null {
  if (!hash.startsWith("#/")) {
    return null;
  }
  const segments = hash.slice(2).split("/");
  const [head, detail] = segments;
  switch (head) {
    case "local":
    case "remote":
      if (segments.length === 1) {
        return { view: head };
      }
      if (segments.length === 2 && detail) {
        const name = decodeSegment(detail);
        return name === null ? null : { view: head, detail: name };
      }
      return null;
    case "discover":
    case "secrets":
      return segments.length === 1 ? { view: head } : null;
    case "profiles":
      return segments.length === 1 ? { view: "profiles" } : null;
    case "settings":
      if (segments.length === 1) {
        return { view: "settings", detail: "system" };
      }
      if (segments.length === 2 && detail) {
        return { view: "settings", detail };
      }
      return null;
    default:
      return null;
  }
}

/** Decodes a percent-encoded segment, or null when the encoding is malformed. */
function decodeSegment(segment: string): string | null {
  try {
    return decodeURIComponent(segment);
  } catch {
    // A malformed escape (a bare "%") is an unknown route, not a crash.
    return null;
  }
}

/** The window surface the router needs; tests hand in a jsdom window. */
export interface RouterWindow {
  /** The location whose hash drives the routes. */
  location: { hash: string };
  /** Event registration for `hashchange`. */
  addEventListener(type: string, listener: () => void): void;
  /** Event removal, so a torn-down router leaves no listener behind. */
  removeEventListener(type: string, listener: () => void): void;
}

/** Mounts one view into `<main>` for a matched route. */
export type ViewMount = (main: HTMLElement, match: RouteMatch) => void | (() => void);

/** Construction options for {@link startRouter}. */
export interface RouterOptions {
  /** The window carrying the hash and the hashchange events. */
  win: RouterWindow;
  /** The `<main>` region the views mount into. */
  main: HTMLElement;
  /** Fired after every render so the tab bar can follow the route. */
  onRoute: (view: ViewId) => void;
  /** Real view mounts by destination; unlisted views render the stub. */
  views?: Partial<Record<ViewId, ViewMount>>;
}

/**
 * Renders the current route now and again on every hash change.
 * Returns the stop function that detaches the hashchange listener, so
 * a shell remount never stacks routers.
 */
export function startRouter(options: RouterOptions): () => void {
  let disposeView: () => void = () => undefined;
  let currentRoute = "";
  const render = () => {
    let match = matchRoute(options.win.location.hash);
    if (!match) {
      // Normalize the address bar; the assignment re-fires hashchange,
      // which re-renders the same view idempotently.
      options.win.location.hash = "#/local";
      match = { view: "local" };
    }
    const routeKey = `${match.view}\0${match.detail ?? ""}`;
    if (routeKey === currentRoute) {
      return;
    }
    currentRoute = routeKey;
    disposeView();
    disposeView = () => undefined;
    const mount = options.views?.[match.view];
    if (mount) {
      const cleanup = mount(options.main, match);
      if (cleanup) {
        disposeView = cleanup;
      }
    } else {
      mountStubView(options.main, match);
    }
    options.onRoute(match.view);
  };
  options.win.addEventListener("hashchange", render);
  render();
  return () => {
    disposeView();
    options.win.removeEventListener("hashchange", render);
  };
}

/** Mounts the stub for `match`: the view name and an empty state. */
function mountStubView(main: HTMLElement, match: RouteMatch): void {
  const title = document.createElement("h1");
  title.className = "view-title";
  title.textContent = VIEW_TITLES[match.view];
  const empty = document.createElement("p");
  empty.className = "view-empty";
  empty.textContent = "Nothing to show here yet.";
  main.replaceChildren(title, empty);
}
