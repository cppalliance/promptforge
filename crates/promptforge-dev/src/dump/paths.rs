//! Maps a logical StoreRef path to a safe relative filesystem path.
//!
//! StoreRef paths use `/` separators. A path is rejected when it is empty or
//! absolute, when any component is empty, `.`, or `..`, when a component
//! carries a separator or a character Windows reserves, or when a component's
//! stem is a reserved device name. This keeps every dumped file inside the
//! dump directory and portable across the hosts a store may be replayed on.

use std::path::PathBuf;

/// Maps one logical store path to a relative filesystem path, or `None` when
/// the path cannot be written safely inside the dump directory.
pub(crate) fn safe_relative_path(path: &str) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let mut relative = PathBuf::new();
    for component in path.split('/') {
        if !component_is_safe(component) {
            return None;
        }
        relative.push(component);
    }
    Some(relative)
}

/// Reports whether one path component is safe as a file or directory name.
fn component_is_safe(component: &str) -> bool {
    if component.is_empty() || component == "." || component == ".." {
        return false;
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return false;
    }
    if component
        .chars()
        .any(|c| c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return false;
    }
    let stem = component.split('.').next().unwrap_or(component);
    !is_reserved_device_name(stem)
}

/// Reports whether `stem` is a Windows reserved device name, which the
/// filesystem would silently redirect rather than store.
fn is_reserved_device_name(stem: &str) -> bool {
    /// One device digit: ASCII `0`-`9` or the legacy superscript digits
    /// `¹ ² ³`, which Windows also treats as device numbers.
    fn is_device_digit(c: char) -> bool {
        c.is_ascii_digit() || matches!(c, '\u{00B9}' | '\u{00B2}' | '\u{00B3}')
    }
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || matches!(upper.strip_prefix("COM"), Some(digit) if digit.chars().count() == 1 && digit.chars().all(is_device_digit))
        || matches!(upper.strip_prefix("LPT"), Some(digit) if digit.chars().count() == 1 && digit.chars().all(is_device_digit))
}

#[cfg(test)]
mod tests {
    use super::safe_relative_path;

    #[test]
    fn safe_relative_paths_accept_ordinary_names_and_reject_escapes() {
        for accepted in ["a.txt", "a/b/c.md", "with space.txt", "dot.in.name"] {
            assert!(
                safe_relative_path(accepted).is_some(),
                "{accepted:?} must be accepted"
            );
        }
        for rejected in [
            "",
            "/a.txt",
            "a//b.txt",
            "..",
            "a/..",
            "../a",
            "a/./b",
            "a\\b",
            "C:/a",
            "a:b",
            "que?.txt",
            "star*.txt",
            "pipe|.txt",
            "quote\".txt",
            "angle<.txt",
            "angle>.txt",
            "ctrl\u{7}.txt",
            "trailing.",
            "trailing ",
            "CON",
            "nul.txt",
            "Com1.log",
            "lpt9",
            "com\u{00B9}",
            "LPT\u{00B3}.txt",
        ] {
            assert!(
                safe_relative_path(rejected).is_none(),
                "{rejected:?} must be rejected"
            );
        }
        for device_lookalike in ["CONSOLE", "COM10", "COMX", "LPT", "nulled.txt"] {
            assert!(
                safe_relative_path(device_lookalike).is_some(),
                "{device_lookalike:?} is not a reserved device name"
            );
        }
    }
}
