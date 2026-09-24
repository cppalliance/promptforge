//! Reports any internal crate name, in kebab or snake spelling, found in
//! the doc text of a surface item or a facade module. Rustdoc inlines a
//! re-exported item's docs into the facade, so a name in them is a name
//! hosts read and cannot use.

use super::Finding;
use super::items::Surface;
use super::load::Loaded;
use super::walk::Visit;

const REQUIRED: &str = "surface doc text that names no internal crate";

/// The doc-text findings for one build's visits and the facade's modules.
pub(crate) fn findings(loaded: &Loaded, surface: &Surface, visits: &[Visit<'_>]) -> Vec<Finding> {
    let names = spellings(surface);
    let mut findings = Vec::new();
    for (label, id) in &surface.modules {
        if let Some(docs) = loaded
            .facade
            .index
            .get(id)
            .and_then(|item| item.docs.as_deref())
        {
            findings.extend(scan(label, docs, &names));
        }
    }
    for visit in visits {
        if let Some(docs) = visit.item.docs.as_deref() {
            findings.extend(scan(&visit.label, docs, &names));
        }
    }
    findings
}

/// Both spellings of every internal crate name, sorted.
fn spellings(surface: &Surface) -> Vec<String> {
    let mut names: Vec<String> = surface
        .internal
        .iter()
        .flat_map(|name| [name.clone(), name.replace('_', "-")])
        .collect();
    names.sort();
    names.dedup();
    names
}

/// One finding per internal crate name in `docs`, at its first line.
fn scan(label: &str, docs: &str, names: &[String]) -> Vec<Finding> {
    names
        .iter()
        .filter_map(|name| {
            let line = docs.lines().position(|line| contains_name(line, name))?;
            Some(Finding {
                item: label.to_owned(),
                mention: format!("internal crate name `{name}` in its doc text"),
                required: REQUIRED.to_owned(),
                found: format!("`{name}` on doc line {}", line + 1),
            })
        })
        .collect()
}

/// Whether `text` holds `name` as a whole crate name: not inside a longer
/// identifier or hyphenated name (`promptforge-lua` is not in
/// `promptforge-luau`).
fn contains_name(text: &str, name: &str) -> bool {
    let part_of_name = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    text.match_indices(name).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + name.len()..].chars().next();
        !before.is_some_and(part_of_name) && !after.is_some_and(part_of_name)
    })
}

#[cfg(test)]
#[path = "doc_text-tests.rs"]
mod tests;
