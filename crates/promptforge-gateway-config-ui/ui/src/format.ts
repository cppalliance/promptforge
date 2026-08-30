// Shared display formatting for byte sizes and file paths. A root
// module (like router.ts) so every layer may import it; extracted when
// the Downloads view would have added a fourth verbatim copy.

/** Human-readable byte size (GiB/MiB/KiB). */
export function formatBytes(bytes: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"] as const;
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

/** The last path segment (either separator); "" for a trailing separator. */
export function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}
