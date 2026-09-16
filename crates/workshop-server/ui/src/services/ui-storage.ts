// The UI-state adapter: localStorage over HTTP. The shell binds the server
// to an OS-assigned loopback port, so the page origin - and with it every
// localStorage entry - changes on each launch. This module replaces that
// storage with two server-backed buckets of opaque JSON values: the
// "workspace" bucket lives in the open .pfwork file (GET and PUT under
// /workspace/file/state), the "user" bucket in the per-user ui-state.json
// (GET and PUT under /user/state). The composition root preloads both
// once before any store is built; stores then read synchronously from the
// cache and write through `set`. Values are opaque to this module and to
// the server: each store owns its own shape, versioning, and validation.
//
// Failure posture is degradation, never blocking: a bucket that fails or
// hangs past the preload timeout reads as all-null (every store falls
// back to its defaults) with one console warning; a failed write warns
// once and the in-memory value stands. `suppressWrites` exists for the
// moment a pulled layout is applied on Open, when the resulting change
// events would otherwise echo the same values straight back to the file.
//
// Stores never import fetch; they take the adapter (or a fake) and tests
// inject `fetchImpl` here.

import { errorText } from "./error-catalog";
import { isRecord } from "./json-request";
import { createServiceToken, registerService } from "./service-registry";

/** Which bucket a value belongs to: the user, or the open workspace file. */
export type Bucket = "user" | "workspace";

/** The SPA's key-value storage over the two server buckets. */
export interface UiStorage {
  /**
   * Fetches both buckets in parallel, each raced against `timeoutMs`, and
   * caches the results. A bucket that fails or times out caches as empty
   * with one warning. Always resolves; never rejects.
   */
  preload(timeoutMs: number): Promise<void>;
  /** The cached value for `key`, or null when absent, null, or never loaded. */
  get(bucket: Bucket, key: string): unknown;
  /**
   * Caches `value` and PUTs it to the bucket's route. Fire-and-forget: a
   * failed write warns once and never throws. Workspace-bucket writes are
   * dropped while `suppressWrites` is active.
   */
  set(bucket: Bucket, key: string, value: unknown): void;
  /** Re-fetches the workspace bucket, replacing its cached values. */
  reloadWorkspace(): Promise<void>;
  /**
   * Runs `fn` with workspace-bucket `set` turned into a no-op, for applying
   * a pulled layout without echoing it back. Nesting is safe; suppression
   * lifts when the outermost call returns or throws.
   */
  suppressWrites<T>(fn: () => T): T;
}

const ROUTES: Readonly<Record<Bucket, string>> = {
  user: "/user/state",
  workspace: "/workspace/file/state",
};

/** Resolves to `null` after `ms`; the loser of a race against a fetch. */
function timeout(ms: number): { promise: Promise<null>; cancel: () => void } {
  let handle: ReturnType<typeof setTimeout> | undefined;
  const promise = new Promise<null>((resolve) => {
    handle = setTimeout(() => resolve(null), ms);
  });
  return {
    promise,
    cancel: () => {
      if (handle !== undefined) {
        clearTimeout(handle);
      }
    },
  };
}

function warn(label: string, detail: string): void {
  console.warn(`ui-storage: ${label} failed: ${detail}`);
}

/**
 * Builds the live adapter. `fetchImpl` defaults to the global fetch; tests
 * pass a scripted one.
 */
export function createUiStorage(fetchImpl: typeof fetch = fetch): UiStorage {
  const cache: Record<Bucket, Map<string, unknown>> = {
    user: new Map(),
    workspace: new Map(),
  };
  let suppressDepth = 0;

  // Fetches one bucket and answers its key-value map, or null after one
  // warning when the transport, the status, or the body shape fails. The
  // warning is skipped once `abandoned()` reports true: the caller has
  // already given up on this fetch (and warned about that), so a late
  // failure is not a second event.
  async function fetchBucket(
    bucket: Bucket,
    abandoned: () => boolean = () => false,
  ): Promise<Map<string, unknown> | null> {
    const label = `GET ${ROUTES[bucket]}`;
    const warnUnlessAbandoned = (detail: string): void => {
      if (!abandoned()) {
        warn(label, detail);
      }
    };
    try {
      const response = await fetchImpl(ROUTES[bucket]);
      if (!response.ok) {
        warnUnlessAbandoned(`answered ${response.status}`);
        return null;
      }
      const body: unknown = await response.json();
      if (!isRecord(body) || Array.isArray(body)) {
        warnUnlessAbandoned("returned a non-object body");
        return null;
      }
      return new Map(Object.entries(body));
    } catch (error) {
      warnUnlessAbandoned(errorText(error));
      return null;
    }
  }

  // Loads one bucket within `timeoutMs`; a late answer after the timeout
  // is dropped, because the stores have already been built from defaults,
  // and the timeout is the bucket's one warning.
  async function loadBucket(bucket: Bucket, timeoutMs: number): Promise<void> {
    const timer = timeout(timeoutMs);
    let settled = false;
    let timedOut = false;
    const guarded = fetchBucket(bucket, () => timedOut).then((result) => {
      settled = true;
      return result;
    });
    const result = await Promise.race([guarded, timer.promise]);
    timer.cancel();
    if (!settled) {
      timedOut = true;
      warn(`GET ${ROUTES[bucket]}`, `no answer within ${timeoutMs} ms`);
    }
    cache[bucket] = result ?? new Map();
  }

  return {
    async preload(timeoutMs: number): Promise<void> {
      await Promise.all([loadBucket("user", timeoutMs), loadBucket("workspace", timeoutMs)]);
    },

    get(bucket: Bucket, key: string): unknown {
      return cache[bucket].get(key) ?? null;
    },

    set(bucket: Bucket, key: string, value: unknown): void {
      if (bucket === "workspace" && suppressDepth > 0) {
        return;
      }
      cache[bucket].set(key, value);
      const route = `${ROUTES[bucket]}/${encodeURIComponent(key)}`;
      const label = `PUT ${route}`;
      let request: Promise<Response>;
      try {
        request = fetchImpl(route, {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(value),
        });
      } catch (error) {
        warn(label, errorText(error));
        return;
      }
      void request
        .then((response) => {
          if (!response.ok) {
            warn(label, `answered ${response.status}`);
          }
        })
        .catch((error: unknown) => {
          warn(label, errorText(error));
        });
    },

    async reloadWorkspace(): Promise<void> {
      cache.workspace = (await fetchBucket("workspace")) ?? new Map();
    },

    suppressWrites<T>(fn: () => T): T {
      suppressDepth += 1;
      try {
        return fn();
      } finally {
        suppressDepth -= 1;
      }
    },
  };
}

/**
 * The adapter with nothing behind it: reads null, writes nowhere, loads
 * instantly. The default under UI_STORAGE, so a consumer that resolves the
 * token before the composition root re-registers the live adapter
 * degrades to defaults instead of caching a wrong instance.
 */
function createEmptyUiStorage(): UiStorage {
  return {
    preload: () => Promise.resolve(),
    get: () => null,
    set: () => {},
    reloadWorkspace: () => Promise.resolve(),
    suppressWrites: (fn) => fn(),
  };
}

/** The registry token for the UI-state adapter. */
export const UI_STORAGE = createServiceToken<UiStorage>("workshop.uiStorage");

// Self-registration with the empty adapter; the composition root
// re-registers the live one after preload.
registerService(UI_STORAGE, () => createEmptyUiStorage());
