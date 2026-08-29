// Validated HTTP boundary for the workspace APIs. Every response arrives
// as unknown and is parsed field by field into a narrow type before any
// consumer touches it; no casts. The tree panel lists directories through
// fetchTree; the editor panel reads and writes files through fetchFile
// and writeFile, which carry the server's opaque conflict token.

/** One entry in a directory listing. */
export interface TreeEntry {
  readonly name: string;
  /** The entry's full path, ready to pass back to the API. */
  readonly path: string;
  readonly kind: "directory" | "file";
  readonly size: number;
  /** Modification time in milliseconds since the Unix epoch. */
  readonly modifiedMs: number;
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

/** Thrown when the server refuses a write because the file changed on disk. */
export class ModifiedConflictError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ModifiedConflictError";
  }
}

/** Narrows a caught error to a modified-time conflict from writeFile. */
export function isModifiedConflict(error: unknown): error is ModifiedConflictError {
  return error instanceof ModifiedConflictError;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function parseEntry(value: unknown): TreeEntry | null {
  if (!isRecord(value)) {
    return null;
  }
  const { name, path, kind, size, modified_ms } = value;
  if (typeof name !== "string" || typeof path !== "string") {
    return null;
  }
  if (kind !== "directory" && kind !== "file") {
    return null;
  }
  if (typeof size !== "number" || typeof modified_ms !== "number") {
    return null;
  }
  return { name, path, kind, size, modifiedMs: modified_ms };
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

/** Extracts the server's error message from a failed workspace response. */
function errorMessage(body: unknown, status: number, route: string): string {
  if (isRecord(body) && isRecord(body.error) && typeof body.error.message === "string") {
    return body.error.message;
  }
  return `${route} answered ${status}`;
}

/**
 * Lists one level of a workspace directory, or the granted roots when
 * `path` is null. Throws on transport, HTTP, and shape failures.
 */
export async function fetchTree(path: string | null): Promise<TreeListing> {
  const route = "/workspace/tree";
  const url = path === null ? route : `${route}?path=${encodeURIComponent(path)}`;
  const response = await fetch(url);
  const body: unknown = await response.json();
  if (!response.ok) {
    throw new Error(errorMessage(body, response.status, `GET ${route}`));
  }
  const listing = parseListing(body);
  if (listing === null) {
    throw new Error(`GET ${route} returned an unexpected shape`);
  }
  return listing;
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

/** The server's machine-readable error code, when the body carries one. */
function errorCode(body: unknown): string | null {
  if (isRecord(body) && isRecord(body.error) && typeof body.error.code === "string") {
    return body.error.code;
  }
  return null;
}

/**
 * Reads a confined workspace file's text with its size and conflict
 * token. Throws on transport, HTTP, and shape failures.
 */
export async function fetchFile(path: string): Promise<WorkspaceFile> {
  const route = "/workspace/file";
  const response = await fetch(`${route}?path=${encodeURIComponent(path)}`);
  const body: unknown = await response.json();
  if (!response.ok) {
    throw new Error(errorMessage(body, response.status, `GET ${route}`));
  }
  const file = parseFile(body);
  if (file === null) {
    throw new Error(`GET ${route} returned an unexpected shape`);
  }
  return file;
}

/**
 * Writes a confined workspace file. `expectedToken` is the token the
 * caller last read, passed back verbatim; the server refuses the write
 * with a 409 when the file changed on disk since, surfaced here as a
 * ModifiedConflictError. Returns the post-write metadata, including the
 * fresh token.
 */
export async function writeFile(
  path: string,
  text: string,
  expectedToken: string | null,
): Promise<WorkspaceFile> {
  const route = "/workspace/file";
  const response = await fetch(route, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path, text, expected_token: expectedToken }),
  });
  const body: unknown = await response.json();
  if (!response.ok) {
    const message = errorMessage(body, response.status, `PUT ${route}`);
    if (response.status === 409 && errorCode(body) === "modified_conflict") {
      throw new ModifiedConflictError(message);
    }
    throw new Error(message);
  }
  const file = parseFile(body);
  if (file === null) {
    throw new Error(`PUT ${route} returned an unexpected shape`);
  }
  return file;
}
