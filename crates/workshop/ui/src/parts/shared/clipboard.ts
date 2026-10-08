// The one clipboard write the Workshop's copy buttons share (the
// transcript's code blocks and turn footer, the About dialog's version
// rows). It reads the browser global at call time, so a test (or a window
// without the API) degrades instead of throwing at import.

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
