// The typed error vocabulary shared by every service and view. Two shapes:
// Result for operations whose callers branch on the outcome (grantPath),
// and CatalogError for the throw-based HTTP boundaries (workspace-api),
// where the ErrorCatalog code - never the message text - is what callers
// match on. Both carry the same stable codes, so a failure's meaning
// survives the trip from the fetch boundary to the status bar.

/** The outcome of one fallible operation: a value, or a typed error. */
export type Result<T, E = CatalogError> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly error: E };

/**
 * The stable machine-readable failure codes. Callers match on these;
 * messages are for humans and may be reworded freely.
 */
export enum ErrorCatalog {
  /** The request never reached a response: DNS, refusal, abort. */
  Transport = "transport",
  /** The server answered with a non-2xx status. */
  HttpStatus = "http_status",
  /** A 2xx answer did not match the wire contract's shape. */
  UnexpectedShape = "unexpected_shape",
  /** The server refused a write because the file changed on disk. */
  ModifiedConflict = "modified_conflict",
  /** The server refused to grant a workspace root. */
  GrantRefused = "grant_refused",
}

/** One typed failure: a catalog code, a human message, the HTTP status. */
export class CatalogError extends Error {
  /** The stable code callers match on. */
  readonly code: ErrorCatalog;
  /** The HTTP status, when a response was received; null on transport. */
  readonly status: number | null;

  constructor(
    code: ErrorCatalog,
    message: string,
    options?: { readonly status?: number; readonly cause?: unknown },
  ) {
    super(message, { cause: options?.cause });
    this.name = "CatalogError";
    this.code = code;
    this.status = options?.status ?? null;
  }
}

/** Wraps a value in a successful Result. */
export function ok<T>(value: T): Result<T, never> {
  return { ok: true, value };
}

/** Wraps an error in a failed Result. */
export function err<E>(error: E): Result<never, E> {
  return { ok: false, error };
}

/** Narrows an unknown catch to a CatalogError, optionally of one code. */
export function isCatalogError(error: unknown, code?: ErrorCatalog): error is CatalogError {
  return (
    error instanceof CatalogError && (code === undefined || error.code === code)
  );
}

/**
 * Renders any caught value as display text: an Error's message, a foreign
 * object's string `message` property (cross-realm errors, WebView bridge
 * failures), or String() as the last resort.
 */
export function errorText(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "object" && error !== null) {
    const message = Reflect.get(error, "message");
    if (typeof message === "string") {
      return message;
    }
  }
  return String(error);
}
