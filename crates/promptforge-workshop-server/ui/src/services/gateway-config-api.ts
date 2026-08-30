// The workshop server's gateway-config surface: the gateway origin the
// config panel's iframe URL is built from, and the narrow server-side
// proxy (/gateway/api/{path}) that forwards the config UI's API calls
// with the gateway bearer key attached. The key lives only in the
// workshop server's process; neither the workshop page nor the iframe
// ever sees it.

/** The fetch signature the helpers use; tests substitute a stub. */
export type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;

/** One API-forward request, as the config panel posts it over the bridge. */
export interface BridgeApiRequest {
  /** Correlation id; echoed on the result message. */
  readonly id: string;
  /** The HTTP method to forward. */
  readonly method: string;
  /** The gateway path (plus query), always starting with "/". */
  readonly path: string;
  /** The JSON request body, or null for a bodyless request. */
  readonly body: string | null;
}

/** The proxy's answer, relayed back to the iframe over the bridge. */
export interface BridgeApiResult {
  /** The gateway's status; 0 means the workshop server was unreachable. */
  readonly status: number;
  /** The gateway's Content-Type, or null when it sent none. */
  readonly contentType: string | null;
  /** The gateway's response body as text. */
  readonly body: string;
}

const defaultFetch: FetchLike = (input, init) => fetch(input, init);

/**
 * Reads the gateway's origin from the workshop server. Any failure -
 * transport, a non-success status, a malformed body - reads as null:
 * without the origin the config panel simply cannot load, which the
 * panel reports itself.
 */
export async function fetchGatewayOrigin(fetchFn: FetchLike = defaultFetch): Promise<string | null> {
  try {
    const response = await fetchFn("/gateway/origin");
    if (!response.ok) {
      return null;
    }
    const data = (await response.json()) as { origin?: unknown };
    return typeof data.origin === "string" && data.origin !== "" ? data.origin : null;
  } catch {
    return null;
  }
}

/**
 * Forwards one bridged API request through the workshop server's
 * /gateway/api proxy, which attaches the bearer key and applies the
 * path allowlist. A transport failure answers status 0, so the iframe's
 * bridge client can surface it as an unreachable gateway rather than
 * hang.
 */
export async function forwardGatewayRequest(
  request: BridgeApiRequest,
  fetchFn: FetchLike = defaultFetch,
): Promise<BridgeApiResult> {
  if (!request.path.startsWith("/")) {
    // Absolute or malformed targets never leave the browser; the server
    // would refuse them anyway, but refusing here keeps the failure local.
    return { status: 403, contentType: null, body: "" };
  }
  const method = request.method.toUpperCase();
  const init: RequestInit = { method };
  if (["POST", "PUT", "PATCH"].includes(method)) {
    // The workshop server's cross-site guard requires body-bearing
    // methods to declare JSON - the bodyless POSTs of the config
    // surface (config-apply, config-revert) included.
    init.headers = { "Content-Type": "application/json" };
  }
  if (request.body !== null) {
    init.body = request.body;
  }
  try {
    const response = await fetchFn(`/gateway/api${request.path}`, init);
    return {
      status: response.status,
      contentType: response.headers.get("content-type"),
      body: await response.text(),
    };
  } catch {
    return { status: 0, contentType: null, body: "" };
  }
}
