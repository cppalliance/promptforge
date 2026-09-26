//! The facade shape check, live: every `.rs` file under the `promptforge`
//! and `harness` facades' `src/` holds only grouping `pub mod` blocks with
//! doc comments, single-item `pub use internal_crate::path::Item;`
//! re-exports, and doc attributes, so every surface item is defined in an
//! internal crate. Runs as part of `cargo test -p build-xtask` and
//! `cargo xtask tidy`.
//!
//! A re-export's path must start at a crate the facade manifest declares
//! under `[dependencies]`. Uniform paths resolve any other head, such as a
//! facade module (`pub use effect::Effect;`), inside the facade, which
//! would give the item a second facade path.
//!
//! Syntax cannot tell a module from a function, since both are snake_case,
//! so a `pub use` whose target is a module passes here; `cargo xtask api`
//! rejects it from the item kinds in rustdoc JSON.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::{Attribute, Item, ItemMod, ItemUse, Meta};
use syn::{UseTree, Visibility};

/// The `promptforge` facade's crate directory, relative to the workspace
/// root.
pub(crate) const FACADE_DIR: [&str; 2] = ["crates", "promptforge"];

/// Every facade the shape check covers, relative to the workspace root.
pub(crate) const FACADE_DIRS: [[&str; 2]; 2] = [FACADE_DIR, ["crates", "harness"]];

const REEXPORT: &str = "a single-item re-export `pub use internal_crate::path::Item;`";
const FACADE_ITEMS: &str = "only grouping `pub mod` blocks and single-item `pub use` re-exports";
const FACADE_ATTRIBUTES: &str = "only doc attributes";

/// Runs the shape check over every facade in [`FACADE_DIRS`], in order.
#[must_use]
pub(crate) fn facade_shape_violations(root: &Path) -> Vec<String> {
    FACADE_DIRS
        .iter()
        .flat_map(|parts| {
            facade_violations(
                &parts
                    .iter()
                    .fold(root.to_path_buf(), |dir, part| dir.join(part)),
            )
        })
        .collect()
}

/// Runs the shape check over every `.rs` file under the facade crate
/// `dir`'s `src/`. A missing crate root, an unreadable manifest, or an
/// unreadable file is reported, not skipped: a file that was never parsed
/// cannot be shown clean.
fn facade_violations(dir: &Path) -> Vec<String> {
    let src = dir.join("src");
    let lib = src.join("lib.rs");
    if !lib.is_file() {
        return vec![format!(
            "{}: the facade has no crate root to check",
            lib.display()
        )];
    }
    let crates = match dependency_crates(&dir.join("Cargo.toml")) {
        Ok(crates) => crates,
        Err(violation) => return vec![violation],
    };
    let mut files = crate::tidy::rust_files(&src);
    files.sort();
    let mut violations = Vec::new();
    for file in &files {
        match fs::read_to_string(file) {
            Ok(text) => violations.extend(source_violations(file, &text, &crates)),
            Err(error) => violations.push(format!(
                "{}: unreadable facade source: {error}",
                file.display()
            )),
        }
    }
    violations
}

/// The crate names a facade re-export path may start at: the keys of the
/// manifest's `[dependencies]` tables, plain and target-specific, with `-`
/// spelled `_` as in code. A key is the crate's name in code even when a
/// `package` rename points it at another package.
fn dependency_crates(manifest: &Path) -> Result<BTreeSet<String>, String> {
    let text = fs::read_to_string(manifest).map_err(|error| {
        format!(
            "{}: unreadable facade manifest: {error}",
            manifest.display()
        )
    })?;
    let value: toml::Value = toml::from_str(&text).map_err(|error| {
        format!(
            "{}: unparseable facade manifest: {error}",
            manifest.display()
        )
    })?;
    Ok(
        crate::manifest::dependency_tables(&value, &["dependencies"])
            .into_iter()
            .flat_map(|(_, table)| table.keys())
            .map(|key| key.replace('-', "_"))
            .collect(),
    )
}

/// The shape violations in one facade source file, labeled with `file`;
/// `crates` are the names a re-export path may start at.
fn source_violations(file: &Path, text: &str, crates: &BTreeSet<String>) -> Vec<String> {
    let mut checker = Checker {
        file,
        crates,
        lines: text.lines().collect(),
        violations: Vec::new(),
    };
    match syn::parse_file(text) {
        Ok(parsed) => {
            checker.attributes(&parsed.attrs);
            checker.items(&parsed.items);
        }
        Err(error) => checker.violations.push(format!(
            "{}:{}: unparseable source: required a file that parses as Rust, found {error}",
            file.display(),
            error.span().start().line
        )),
    }
    checker.violations
}

struct Checker<'a> {
    file: &'a Path,
    crates: &'a BTreeSet<String>,
    lines: Vec<&'a str>,
    violations: Vec<String>,
}

impl Checker<'_> {
    /// Records a violation at `span`'s line, quoting that source line as
    /// what was found.
    fn report(&mut self, span: Span, rule: &str, required: &str) {
        let line = span.start().line;
        let found = line
            .checked_sub(1)
            .and_then(|index| self.lines.get(index))
            .map_or("", |text| text.trim());
        self.violations.push(format!(
            "{}:{line}: {rule}: required {required}, found `{found}`",
            self.file.display()
        ));
    }

    fn items(&mut self, items: &[Item]) {
        for item in items {
            match item {
                Item::Use(item) => self.reexport(item),
                Item::Mod(item) => self.module(item),
                other => {
                    let (kind, span) = definition(other);
                    self.report(span, &format!("item definition ({kind})"), FACADE_ITEMS);
                }
            }
        }
    }

    fn reexport(&mut self, item: &ItemUse) {
        self.attributes(&item.attrs);
        let rule = if matches!(item.vis, Visibility::Public(_)) {
            reexport_rule(&item.tree, self.crates)
        } else {
            Some("non-public use")
        };
        if let Some(rule) = rule {
            self.report(item.use_token.span, rule, REEXPORT);
        }
    }

    fn module(&mut self, item: &ItemMod) {
        self.attributes(&item.attrs);
        if !matches!(item.vis, Visibility::Public(_)) {
            self.report(
                item.mod_token.span,
                "non-public module",
                "a grouping `pub mod`",
            );
        } else if !item.attrs.iter().any(is_doc_comment) {
            self.report(
                item.mod_token.span,
                "undocumented module",
                "a doc comment on every grouping `pub mod`",
            );
        }
        if let Some((_, items)) = &item.content {
            self.items(items);
        }
    }

    fn attributes(&mut self, attrs: &[Attribute]) {
        for attr in attrs {
            if !attr.path().is_ident("doc") {
                self.report(attr.span(), "disallowed attribute", FACADE_ATTRIBUTES);
            }
        }
    }
}

/// The rule a public `use` tree breaks, or `None` for a single item named
/// through a path that starts at one of `crates`.
fn reexport_rule(tree: &UseTree, crates: &BTreeSet<String>) -> Option<&'static str> {
    let mut head = None;
    let mut tail = tree;
    while let UseTree::Path(path) = tail {
        head.get_or_insert(&path.ident);
        tail = &path.tree;
    }
    match (tail, head) {
        (UseTree::Glob(_), _) => Some("glob re-export"),
        (UseTree::Group(_), _) => Some("grouped use list"),
        (_, None) => Some("crate re-export"),
        (_, Some(head)) if *head == "crate" || *head == "self" || *head == "super" => {
            Some("re-export of a facade path")
        }
        (_, Some(head)) if !crates.iter().any(|name| head == name) => {
            Some("re-export not rooted at a facade dependency")
        }
        _ => None,
    }
}

/// The kind of an item the facade may not define, and the span of the
/// token that names it (past any attributes, so the line is the item's own).
fn definition(item: &Item) -> (&'static str, Span) {
    match item {
        Item::Const(item) => ("const", item.ident.span()),
        Item::Enum(item) => ("enum", item.ident.span()),
        Item::ExternCrate(item) => ("extern crate", item.ident.span()),
        Item::Fn(item) => ("fn", item.sig.ident.span()),
        Item::ForeignMod(item) => ("extern block", item.abi.extern_token.span),
        Item::Impl(item) => ("impl", item.impl_token.span),
        Item::Macro(item) => match &item.ident {
            Some(ident) => ("macro_rules!", ident.span()),
            None => ("macro invocation", item.mac.path.span()),
        },
        Item::Static(item) => ("static", item.ident.span()),
        Item::Struct(item) => ("struct", item.ident.span()),
        Item::Trait(item) => ("trait", item.ident.span()),
        Item::TraitAlias(item) => ("trait alias", item.ident.span()),
        Item::Type(item) => ("type alias", item.ident.span()),
        Item::Union(item) => ("union", item.ident.span()),
        other => ("item syn does not model", other.span()),
    }
}

/// Whether an attribute is a doc comment (`///`, `//!`, or `#[doc = ...]`),
/// as opposed to a doc setting such as `#[doc(hidden)]`.
fn is_doc_comment(attr: &Attribute) -> bool {
    attr.path().is_ident("doc") && matches!(attr.meta, Meta::NameValue(_))
}

#[cfg(test)]
#[path = "facade_shape-tests.rs"]
mod tests;
