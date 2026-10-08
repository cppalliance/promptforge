// Motion and clipboard helpers shared by the transcript's row components.
// Both read browser globals at call time, so a test (or a window without
// the API) degrades instead of throwing at import.

/** True when the operator asked the system to reduce motion. */
export function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

/**
 * Puts `text` on the system clipboard. Resolves true once it landed, false
 * when the page has no clipboard or the write was refused; callers show
 * their confirmation only on true.
 */
export async function copyToClipboard(text: string): Promise<boolean> {
  const clipboard = typeof navigator === "undefined" ? undefined : navigator.clipboard;
  if (clipboard === undefined || typeof clipboard.writeText !== "function") {
    return false;
  }
  try {
    await clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
