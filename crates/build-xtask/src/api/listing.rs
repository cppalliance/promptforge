//! The surface listing: one sorted line per surface item, including the
//! methods, fields, variants, and trait impls of every re-exported type,
//! built from the default build only because that is the surface hosts
//! get. It is committed as `crates/promptforge/public-api.txt`, so every
//! change to the surface shows up in review as a change to that file.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::items::Surface;
use super::load::FACADE;
use super::render::Renderer;
use super::walk::Visit;

/// The committed listing's path under the workspace root.
pub(crate) fn path(root: &Path) -> PathBuf {
    crate::facade_shape::FACADE_DIR
        .iter()
        .fold(root.to_path_buf(), |dir, part| dir.join(part))
        .join("public-api.txt")
}

/// The listing lines for one build, sorted and deduplicated.
pub(crate) fn lines(surface: &Surface, visits: &[Visit<'_>]) -> Vec<String> {
    let mut lines: BTreeSet<String> = surface
        .modules
        .iter()
        .filter(|(label, _)| label != FACADE)
        .map(|(label, _)| format!("pub mod {label}"))
        .collect();
    for visit in visits {
        if let Some(line) = Renderer::new(visit.krate, surface).line(visit) {
            lines.insert(line);
        }
    }
    lines.into_iter().collect()
}

/// The listing file's text: each line newline-terminated.
pub(crate) fn text(lines: &[String]) -> String {
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    text
}

/// The report lines for how `committed` (the file's text, or `None` when
/// it is missing) differs from `lines`; empty when they match exactly.
/// Line endings are compared as `\n`, whatever the checkout wrote.
pub(crate) fn difference(committed: Option<&str>, lines: &[String]) -> Vec<String> {
    let committed_text = committed.map(|text| text.replace("\r\n", "\n"));
    if committed_text.as_deref() == Some(text(lines).as_str()) {
        return Vec::new();
    }
    let old: BTreeSet<&str> = committed_text.as_deref().unwrap_or("").lines().collect();
    let new: BTreeSet<&str> = lines.iter().map(String::as_str).collect();
    let mut report = vec![match committed {
        Some(_) => "public-api.txt differs from the surface listing:".to_owned(),
        None => "public-api.txt is missing; the surface listing is:".to_owned(),
    }];
    report.extend(old.difference(&new).map(|line| format!("- {line}")));
    report.extend(new.difference(&old).map(|line| format!("+ {line}")));
    if report.len() == 1 {
        report.push("  the same lines, not sorted or not newline-terminated".to_owned());
    }
    report
}

#[cfg(test)]
#[path = "listing-tests.rs"]
mod tests;
