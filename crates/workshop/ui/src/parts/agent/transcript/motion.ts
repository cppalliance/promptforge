// The motion preference shared by the transcript's row components. It
// reads the browser global at call time, so a test (or a window without
// the API) degrades instead of throwing at import.

/** True when the operator asked the system to reduce motion. */
export function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}
