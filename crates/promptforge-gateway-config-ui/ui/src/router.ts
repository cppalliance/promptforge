// Hash router [Adapted: llama.cpp] for both shell modes. Routes:
// #/models, #/models/{name}, #/discover, #/profiles, #/secrets,
// #/settings/{section}; anything else falls back to
// #/models. Each route mounts its view into <main>; until the real
// views land, the mounts are stubs rendering the view name and an
// empty state.

/** The five top-level destinations. */
export type ViewId = "models" | "discover" | "profiles" | "secrets" | "settings";

/** A parsed route: the view plus its optional detail segment. */
export interface RouteMatch {
  /** The destination view. */
  view: ViewId;
  /** The model name or settings section, when the route carries one. */
  detail?: string;
}

/** Display titles for the stub views. */
const VIEW_TITLES: Readonly<Record<ViewId, string>> = {
  models: "Models",
  discover: "Discover",
  profiles: "Profiles",
  secrets: "Secrets",
  settings: "Settings",
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
    case "models":
      if (segments.length === 1) {
        return { view: "models" };
      }
      if (segments.length === 2 && detail) {
        const name = decodeSegment(detail);
        return name === null ? null : { view: "models", detail: name };
      }
      return null;
    case "discover":
    case "secrets":
      return segments.length === 1 ? { view: head } : null;
    case "profiles":
      if (segments.length === 1) {
        return { view: "profiles" };
      }
      // #/profiles/include/{encoded-path}: the include-file drill-in.
      if (segments.length === 3 && detail === "include" && segments[2]) {
        const path = decodeSegment(segments[2]);
        return path === null ? null : { view: "profiles", detail: path };
      }
      return null;
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
export type ViewMount = (main: HTMLElement, match: RouteMatch) => void;

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
  const render = () => {
    let match = matchRoute(options.win.location.hash);
    if (!match) {
      // Normalize the address bar; the assignment re-fires hashchange,
      // which re-renders the same view idempotently.
      options.win.location.hash = "#/models";
      match = { view: "models" };
    }
    const mount = options.views?.[match.view];
    if (mount) {
      mount(options.main, match);
    } else {
      mountStubView(options.main, match);
    }
    options.onRoute(match.view);
  };
  options.win.addEventListener("hashchange", render);
  render();
  return () => options.win.removeEventListener("hashchange", render);
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
