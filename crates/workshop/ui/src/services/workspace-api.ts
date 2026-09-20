// Validated HTTP boundary for the workspace APIs. Every response arrives
// as unknown and is parsed field by field into a narrow type before any
// consumer touches it; no casts. The tree panel lists directories through
// fetchTree; the editor panel reads and writes files through fetchFile
// and writeFile, which carry the server's opaque conflict token. Failures
// throw typed CatalogError variants (services/error-catalog.ts); callers
// match on the ErrorCatalog code, never on message text. The fetch and
// JSON mechanics are the shared floor in json-request.ts.

import { CatalogError, ErrorCatalog, isCatalogError } from "./error-catalog";
import { errorCode, errorMessage, isRecord, readJson, request } from "./json-request";

/** One entry in a directory listing. */
export interface TreeEntry {
  readonly name: string;
  /** The entry's full path, ready to pass back to the API. */
  readonly path: string;
  readonly kind: "directory" | "file";
  readonly size: number;
  /** Modification time in milliseconds since the Unix epoch. */
  readonly modifiedMs: number;
  /** False only for a granted root that no longer exists on disk. */
  readonly exists: boolean;
}

/** One level of a workspace directory tree. */
export interface TreeListing {
  /** The listed directory; null when the listing is the granted roots. */
  readonly path: string | null;
  /** Directories before files, each group ordered by name. */
  readonly entries: readonly TreeEntry[];
}

/** A file's text plus the metadata a writer needs to detect conflicts. */
export interface WorkspaceFile {
  readonly path: string;
  readonly size: number;
  /** The server's opaque conflict token; echoed back verbatim on write. */
  readonly token: string | null;
  readonly text: string;
}

/** Narrows a caught error to a modified-time conflict from writeFile. */
export function isModifiedConflict(error: unknown): error is CatalogError {
  return isCatalogError(error, ErrorCatalog.ModifiedConflict);
}

function parseEntry(value: unknown): TreeEntry | null {
  if (!isRecord(value)) {
    return null;
  }
  const { name, path, kind, size, modified_ms, exists } = value;
  if (typeof name !== "string" || typeof path !== "string") {
    return null;
  }
  if (kind !== "directory" && kind !== "file") {
    return null;
  }
  if (typeof size !== "number" || typeof modified_ms !== "number") {
    return null;
  }
  if (typeof exists !== "boolean") {
    return null;
  }
  return { name, path, kind, size, modifiedMs: modified_ms, exists };
}

function parseListing(body: unknown): TreeListing | null {
  if (!isRecord(body)) {
    return null;
  }
  const { path, entries } = body;
  if (path !== null && typeof path !== "string") {
    return null;
  }
  if (!Array.isArray(entries)) {
    return null;
  }
  const parsed: TreeEntry[] = [];
  for (const entry of entries) {
    const parsedEntry = parseEntry(entry);
    if (parsedEntry === null) {
      return null;
    }
    parsed.push(parsedEntry);
  }
  return { path: path ?? null, entries: parsed };
}

/** Throws the typed failure for one non-OK response. */
function httpFailure(body: unknown, status: number, route: string): never {
  const message = errorMessage(body, status, route);
  if (status === 409 && errorCode(body) === "modified_conflict") {
    throw new CatalogError(ErrorCatalog.ModifiedConflict, message, { status });
  }
  throw new CatalogError(ErrorCatalog.HttpStatus, message, { status });
}

/**
 * Lists one level of a workspace directory, or the granted roots when
 * `path` is null. Throws typed CatalogError variants on transport, HTTP,
 * and shape failures.
 */
export async function fetchTree(path: string | null): Promise<TreeListing> {
  const route = "/workspace/tree";
  const url = path === null ? route : `${route}?path=${encodeURIComponent(path)}`;
  const response = await request(url, `GET ${route}`);
  const body = await readJson(response, `GET ${route}`);
  if (!response.ok) {
    httpFailure(body, response.status, `GET ${route}`);
  }
  const listing = parseListing(body);
  if (listing === null) {
    throw new CatalogError(ErrorCatalog.UnexpectedShape, `GET ${route} returned an unexpected shape`);
  }
  return listing;
}

/**
 * Removes a granted root by its listed path. The server refuses unknown
 * roots with a 404; a root deleted from disk stays revocable. Throws
 * typed CatalogError variants on transport and HTTP failures.
 */
export async function revokeRoot(path: string): Promise<void> {
  const route = "/workspace/revoke";
  const response = await request(route, `POST ${route}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path }),
  });
  const body = await readJson(response, `POST ${route}`);
  if (!response.ok) {
    httpFailure(body, response.status, `POST ${route}`);
  }
}

function parseFile(body: unknown): WorkspaceFile | null {
  if (!isRecord(body)) {
    return null;
  }
  const { path, size, token, text } = body;
  if (typeof path !== "string" || typeof text !== "string") {
    return null;
  }
  if (typeof size !== "number") {
    return null;
  }
  if (token !== null && typeof token !== "string") {
    return null;
  }
  return { path, size, token, text };
}

/**
 * Reads a confined workspace file's text with its size and conflict
 * token. Throws typed CatalogError variants on transport, HTTP, and
 * shape failures.
 */
export async function fetchFile(path: string): Promise<WorkspaceFile> {
  const route = "/workspace/file";
  const response = await request(`${route}?path=${encodeURIComponent(path)}`, `GET ${route}`);
  const body = await readJson(response, `GET ${route}`);
  if (!response.ok) {
    httpFailure(body, response.status, `GET ${route}`);
  }
  const file = parseFile(body);
  if (file === null) {
    throw new CatalogError(ErrorCatalog.UnexpectedShape, `GET ${route} returned an unexpected shape`);
  }
  return file;
}

/**
 * Writes a confined workspace file. `expectedToken` is the token the
 * caller last read, passed back verbatim; the server refuses the write
 * with a 409 when the file changed on disk since, surfaced here as a
 * CatalogError with the ModifiedConflict code (see isModifiedConflict).
 * Returns the post-write metadata, including the fresh token.
 */
export async function writeFile(
  path: string,
  text: string,
  expectedToken: string | null,
): Promise<WorkspaceFile> {
  const route = "/workspace/file";
  const response = await request(route, `PUT ${route}`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path, text, expected_token: expectedToken }),
  });
  const body = await readJson(response, `PUT ${route}`);
  if (!response.ok) {
    httpFailure(body, response.status, `PUT ${route}`);
  }
  const file = parseFile(body);
  if (file === null) {
    throw new CatalogError(ErrorCatalog.UnexpectedShape, `PUT ${route} returned an unexpected shape`);
  }
  return file;
}
