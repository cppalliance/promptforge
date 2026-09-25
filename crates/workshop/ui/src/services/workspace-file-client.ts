// Validated HTTP boundary for the workspace-file APIs: the workspace as
// a document. GET current, POST open / save_as /
// duplicate, and PUT window-state, every one same-origin. Every switch
// answers with the workspace as it now stands, so the caller never needs
// a second round trip to learn what it switched to. Responses arrive as
// unknown and are parsed field by field into narrow types before any
// consumer touches them; no casts. Failures throw typed CatalogError
// variants (services/error-catalog.ts); callers match on the code, never
// on message text. Same discipline as workspace-api.ts, over the same
// fetch and JSON floor in json-request.ts.

import { CatalogError, ErrorCatalog } from "./error-catalog";
import { errorMessage, isRecord, readJson, request } from "./json-request";

/** One granted root as the workspace file lists it. */
export interface WorkspaceGrant {
  /** The canonical granted root. */
  readonly path: string;
  /** False only for a granted root that no longer exists on disk. */
  readonly exists: boolean;
}

/** The desktop app's saved window geometry, in logical pixels. */
export interface WindowState {
  readonly width: number;
  readonly height: number;
  readonly x: number;
  readonly y: number;
  readonly maximized: boolean;
}

/**
 * The workspace as it stands: `GET /workspace/file/current` and the
 * answer to every successful switch. Mirrors the server's
 * WorkspaceFileResponse; the wire's `window_state` lands on windowState.
 */
export interface WorkspaceFileResponse {
  /** The backing file; null while the workspace is ephemeral. */
  readonly path: string | null;
  /** The display name: the file's own, or "Untitled" while ephemeral. */
  readonly name: string;
  /** The granted roots in canonical order. */
  readonly grants: readonly WorkspaceGrant[];
  /** The saved geometry; null while ephemeral or never saved. */
  readonly windowState: WindowState | null;
}

function parseGrant(value: unknown): WorkspaceGrant | null {
  if (!isRecord(value)) {
    return null;
  }
  const { path, exists } = value;
  if (typeof path !== "string" || typeof exists !== "boolean") {
    return null;
  }
  return { path, exists };
}

function parseWindowState(value: unknown): WindowState | null {
  if (!isRecord(value)) {
    return null;
  }
  const { width, height, x, y, maximized } = value;
  if (typeof width !== "number" || typeof height !== "number") {
    return null;
  }
  if (typeof x !== "number" || typeof y !== "number" || typeof maximized !== "boolean") {
    return null;
  }
  return { width, height, x, y, maximized };
}

function parseWorkspaceFile(body: unknown): WorkspaceFileResponse | null {
  if (!isRecord(body)) {
    return null;
  }
  const { path, name, grants, window_state } = body;
  if (path !== null && typeof path !== "string") {
    return null;
  }
  if (typeof name !== "string" || !Array.isArray(grants)) {
    return null;
  }
  const parsedGrants: WorkspaceGrant[] = [];
  for (const grant of grants) {
    const parsed = parseGrant(grant);
    if (parsed === null) {
      return null;
    }
    parsedGrants.push(parsed);
  }
  let windowState: WindowState | null = null;
  if (window_state !== null && window_state !== undefined) {
    windowState = parseWindowState(window_state);
    if (windowState === null) {
      return null;
    }
  }
  return { path: path ?? null, name, grants: parsedGrants, windowState };
}

/**
 * Performs one request and reads its JSON body, wrapping transport
 * failures, non-JSON answers, and non-OK statuses as typed errors. Every
 * route here answers a plain HttpStatus on refusal; none reports a
 * distinguished code the way the write route's modified_conflict does.
 */
async function requestJson(route: string, init?: RequestInit): Promise<unknown> {
  const label = `${init?.method ?? "GET"} ${route}`;
  const response = await request(route, label, init);
  const body = await readJson(response, label);
  if (!response.ok) {
    throw new CatalogError(ErrorCatalog.HttpStatus, errorMessage(body, response.status, label), {
      status: response.status,
    });
  }
  return body;
}

/** Parses one workspace answer, refusing an unexpected shape. */
function workspaceFrom(body: unknown, label: string): WorkspaceFileResponse {
  const parsed = parseWorkspaceFile(body);
  if (parsed === null) {
    throw new CatalogError(ErrorCatalog.UnexpectedShape, `${label} returned an unexpected shape`);
  }
  return parsed;
}

/** Posts one path to a switch route and answers the workspace as switched. */
async function switchTo(route: string, path: string): Promise<WorkspaceFileResponse> {
  const body = await requestJson(route, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path }),
  });
  return workspaceFrom(body, `POST ${route}`);
}

/** The workspace as it stands: its file, name, grants, and saved geometry. */
export async function currentWorkspaceFile(): Promise<WorkspaceFileResponse> {
  const route = "/workspace/file/current";
  return workspaceFrom(await requestJson(route), `GET ${route}`);
}

/**
 * Opens a workspace file, replacing every grant with its contents. The
 * server refuses an alien or unsupported file and changes nothing.
 */
export function openWorkspaceFile(path: string): Promise<WorkspaceFileResponse> {
  return switchTo("/workspace/file/open", path);
}

/**
 * Creates a new workspace file at `path` holding the current grants and
 * switches to it; a taken path is refused as a conflict.
 */
export function saveWorkspaceFileAs(path: string): Promise<WorkspaceFileResponse> {
  return switchTo("/workspace/file/save_as", path);
}

/** Copies the current workspace file to `path` and switches to the copy. */
export function duplicateWorkspaceFile(path: string): Promise<WorkspaceFileResponse> {
  return switchTo("/workspace/file/duplicate", path);
}

/**
 * Saves the desktop app's window geometry into the open workspace file.
 * Answers whether it was written: false when the workspace is ephemeral
 * and has nowhere to keep it.
 */
export async function putWindowState(state: WindowState): Promise<boolean> {
  const route = "/workspace/file/window-state";
  const body = await requestJson(route, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(state),
  });
  if (!isRecord(body) || typeof body.saved !== "boolean") {
    throw new CatalogError(ErrorCatalog.UnexpectedShape, `PUT ${route} returned an unexpected shape`);
  }
  return body.saved;
}
