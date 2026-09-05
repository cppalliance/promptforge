// Shared JSON boundary helpers for the services layer and its
// consumers: untrusted values arriving over the wire are narrowed
// here before any field is read.

/** Whether untrusted JSON is a non-array object. */
export function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
