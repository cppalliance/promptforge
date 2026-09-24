//! The closure check: every path a surface item's rendering names, and
//! every target of its resolved intra-doc links, must be a facade
//! re-export, std, core, alloc, or an allowlisted crate.
//!
//! A link may also target a member of a surface item (a method, field,
//! variant, or associated item), which rustdoc resolves to the member's
//! own path or, when the member has none in the path table, to an id the
//! walk visited.

use std::collections::HashSet;

use rustdoc_types::{Crate, Id, Item, ItemKind};

use super::Finding;
use super::items::{Surface, required, target};
use super::load::{FACADE, Loaded};
use super::render::Renderer;
use super::walk::Visit;

/// The closure findings for one build's visits and the facade's modules.
pub(crate) fn findings(loaded: &Loaded, surface: &Surface, visits: &[Visit<'_>]) -> Vec<Finding> {
    let visited: HashSet<(&str, Id)> = visits
        .iter()
        .map(|visit| (visit.crate_name, visit.item.id))
        .collect();
    let mut findings = Vec::new();
    for visit in visits {
        let mut renderer = Renderer::new(visit.krate, surface);
        renderer.line(visit);
        for mention in renderer.mentions {
            let refusal = match &mention.target {
                Some((krate, path)) => surface
                    .refusal(krate, path)
                    .map(|found| (path.join("::"), found)),
                None => Some((
                    mention.written.clone(),
                    "a path rustdoc could not resolve".to_owned(),
                )),
            };
            if let Some((shown, found)) = refusal {
                findings.push(Finding {
                    item: visit.label.clone(),
                    mention: format!("`{shown}` in its {}", mention.context),
                    required: required(),
                    found,
                });
            }
        }
        let links = Links {
            surface,
            krate: visit.krate,
            crate_name: visit.crate_name,
            visited: &visited,
        };
        findings.extend(links.findings(&visit.label, visit.item));
    }
    for (label, id) in &surface.modules {
        if let Some(item) = loaded.facade.index.get(id) {
            let links = Links {
                surface,
                krate: &loaded.facade,
                crate_name: FACADE,
                visited: &visited,
            };
            findings.extend(links.findings(label, item));
        }
    }
    findings
}

/// Checks the resolved intra-doc links of items from one crate's JSON.
struct Links<'a> {
    surface: &'a Surface,
    krate: &'a Crate,
    crate_name: &'a str,
    visited: &'a HashSet<(&'a str, Id)>,
}

impl Links<'_> {
    fn findings(&self, label: &str, item: &Item) -> Vec<Finding> {
        let mut links: Vec<(&String, &Id)> = item.links.iter().collect();
        links.sort();
        let mut findings = Vec::new();
        for (text, id) in links {
            let refusal = match target(self.krate, *id) {
                Some(found) => self
                    .refusal(found.krate, found.path, found.kind)
                    .map(|why| (found.path.join("::"), why)),
                None if self.visited.contains(&(self.crate_name, *id)) => None,
                None => Some((text.clone(), "a target rustdoc gives no path".to_owned())),
            };
            if let Some((shown, found)) = refusal {
                findings.push(Finding {
                    item: label.to_owned(),
                    mention: format!("`{shown}` through its doc link `[{text}]`"),
                    required: required(),
                    found,
                });
            }
        }
        findings
    }

    /// A member's link passes when its owner is on the surface, since
    /// rustdoc names members by their owner's path plus their own name.
    fn refusal(&self, krate: &str, path: &[String], kind: ItemKind) -> Option<String> {
        let refusal = self.surface.refusal(krate, path)?;
        let member = matches!(
            kind,
            ItemKind::Function
                | ItemKind::StructField
                | ItemKind::Variant
                | ItemKind::AssocConst
                | ItemKind::AssocType
        );
        let owner = path.split_last().map(|(_, owner)| owner);
        match owner {
            Some(owner) if member && self.surface.facade_path(krate, owner).is_some() => None,
            _ => Some(refusal),
        }
    }
}

#[cfg(test)]
#[path = "closure-tests.rs"]
mod tests;
