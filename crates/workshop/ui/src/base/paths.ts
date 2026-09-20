// Path string helpers shared across features. These operate on the
// workspace's opaque path strings (forward or backslash separated);
// they never touch a filesystem.

/** The file's base name: the last segment after a slash or backslash. */
export function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}
