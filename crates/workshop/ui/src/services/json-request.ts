// The shared transport floor under the validated HTTP boundaries
// (workspace-api.ts, workspace-file-client.ts): one fetch wrapper, one
// JSON reader, and the readers of the server's error envelope
// `{ error: { code, message } }`. Each client keeps its own field-by-
// field body parsers and its own non-OK policy on top of these; only the
// mechanics that every same-origin JSON client repeats live here.
// Failures throw typed CatalogError variants (services/error-catalog.ts).

import { CatalogError, ErrorCatalog, errorText } from "./error-catalog";

/** Narrows an unknown value to a plain object whose fields can be read. */
export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/** Extracts the server's error message from a failed response body. */
export function errorMessage(body: unknown, status: number, route: string): string {
  if (isRecord(body) && isRecord(body.error) && typeof body.error.message === "string") {
    return body.error.message;
  }
  return `${route} answered ${status}`;
}

/** The server's machine-readable error code, when the body carries one. */
export function errorCode(body: unknown): string | null {
  if (isRecord(body) && isRecord(body.error) && typeof body.error.code === "string") {
    return body.error.code;
  }
  return null;
}

/**
 * Performs one fetch, wrapping a transport failure as a typed error.
 * `route` labels the failure (for example "GET /workspace/tree") and may
 * differ from `url` when the URL carries a query string.
 */
export async function request(url: string, route: string, init?: RequestInit): Promise<Response> {
  try {
    return await fetch(url, init);
  } catch (error) {
    throw new CatalogError(ErrorCatalog.Transport, `${route}: ${errorText(error)}`, {
      cause: error,
    });
  }
}

/** Parses one response body; a non-JSON answer is a shape failure. */
export async function readJson(response: Response, route: string): Promise<unknown> {
  try {
    return await response.json();
  } catch (error) {
    throw new CatalogError(ErrorCatalog.UnexpectedShape, `${route} returned a non-JSON answer`, {
      status: response.status,
      cause: error,
    });
  }
}
