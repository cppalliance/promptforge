// Validated HTTP boundary for the workspace APIs. Every response arrives
// as unknown and is parsed field by field into a narrow type before any
// consumer touches it; no casts. The tree panel lists directories through
// here, and step 14's editor adds read/write calls beside fetchTree.

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
