//! The `promptforge:file-drop` dispatch shared by both drop paths: the
//! Windows WebView2 web-message bridge ([`crate::bridge`]) and Tauri's own
//! drag-drop window event everywhere else.
//!
//! The page grants each path through the workspace HTTP API; the app never
//! reads file bytes merely because a file was dragged onto the window.

use std::path::{Path, PathBuf};

/// Renders a dropped OS path for the browser event: the path's own text
/// with any Windows verbatim (`\\?\`) prefix stripped, so the page hands
/// the workspace API the same spelling Explorer shows. The verbatim UNC
/// form (`\\?\UNC\server\share`) collapses to the plain UNC spelling
/// (`\\server\share`). Separators stay native - the workspace server runs
/// on this same machine and canonicalizes whatever it receives.
#[must_use]
pub(crate) fn normalize_dropped_path(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    match text.strip_prefix(r"\\?\") {
        Some(stripped) => stripped.to_owned(),
        None => text,
    }
}

/// Dispatches the `promptforge:file-drop` event carrying the normalized
/// dropped paths. The page listens for the event and grants each path
/// through the workspace HTTP API.
pub(crate) fn dispatch_file_drop(window: &tauri::WebviewWindow, paths: &[PathBuf]) {
    let normalized: Vec<String> = paths
        .iter()
        .map(|path| normalize_dropped_path(path))
        .collect();
    let detail = match serde_json::to_string(&normalized) {
        Ok(detail) => detail,
        Err(error) => {
            eprintln!("could not encode the dropped paths: {error}");
            return;
        }
    };
    let script = format!(
        "window.dispatchEvent(new CustomEvent(\"promptforge:file-drop\", {{detail: {{paths: {detail}}}}}));"
    );
    if let Err(error) = window.eval(&script) {
        eprintln!("could not dispatch the file-drop event: {error}");
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::normalize_dropped_path;

    #[test]
    fn dropped_paths_keep_backslashes_spaces_and_unicode() {
        for path in [
            r"C:\Users\Vinnie\Documents\project",
            r"C:\Users\Vinnie\My Documents\file name.txt",
            "C:\\Users\\Vinnie\\caf\u{e9} \u{4e2d}\u{6587}.txt",
            r"D:\src\promptforge\crates",
        ] {
            assert_eq!(normalize_dropped_path(Path::new(path)), path, "{path}");
        }
    }

    #[test]
    fn dropped_paths_shed_the_verbatim_prefix() {
        assert_eq!(
            normalize_dropped_path(Path::new(r"\\?\C:\Users\Vinnie\file.txt")),
            r"C:\Users\Vinnie\file.txt"
        );
        assert_eq!(
            normalize_dropped_path(Path::new("\\\\?\\D:\\src\\caf\u{e9}.txt")),
            "D:\\src\\caf\u{e9}.txt"
        );
        assert_eq!(
            normalize_dropped_path(Path::new(r"\\?\UNC\server\share\file.txt")),
            r"\\server\share\file.txt"
        );
    }
}
