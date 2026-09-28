// Path string helpers shared across features. These operate on the
// workspace's opaque path strings (forward or backslash separated);
// they never touch a filesystem.

/** The last non-empty segment after a slash or backslash, or undefined for an empty or root path. */
export function lastSegment(path: string): string | undefined {
  return path.split(/[\\/]/).filter(Boolean).pop();
}

/** The file's base name: the last segment after a slash or backslash. */
export function baseName(path: string): string {
  return lastSegment(path) ?? path;
}
