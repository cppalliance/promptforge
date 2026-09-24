//! The `doc(hidden)` ban, live: no `.rs` file under the `promptforge`
//! facade or the `crates/promptforge-internal/` container carries a
//! `doc(hidden)` attribute. An engine item is either on the facade's
//! surface and documented, or off it: private, or a free function in a
//! `detail` module the facade never re-exports. Runs as part of
//! `cargo test -p build-xtask` and `cargo xtask tidy`.
//!
//! Each file must parse with `syn`; then every attribute in its token
//! tree is checked, outer or inner, at any depth. That covers items,
//! fields, variants, methods, and re-exports, and also the tokens inside
//! a macro, where `syn` sees no items but a `macro_rules!` body or a
//! declaring macro's input still hides what it expands to. `doc(hidden)`,
//! a `doc` list naming `hidden` among other settings, and a `cfg_attr`
//! that applies either are all rejected.

use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::{Delimiter, TokenStream, TokenTree};

const RULE: &str = "doc(hidden) attribute";
const REQUIRED: &str = "no hidden item in the engine crates: document it, or make it private \
    or a `detail` function";

/// The directories the ban covers: the facade crate and the engine
/// container, whether or not they exist.
fn banned_dirs(root: &Path) -> [PathBuf; 2] {
    let facade = crate::facade_shape::FACADE_DIR
        .iter()
        .fold(root.to_path_buf(), |dir, part| dir.join(part));
    let container = root
        .join("crates")
        .join(crate::engine_guards::ENGINE_CONTAINER);
    [facade, container]
}

/// Runs the ban over every `.rs` file under the facade and the engine
/// container. A missing directory, an unreadable file, or an unparseable
/// file is reported, not skipped: a file that was never scanned cannot be
/// shown clean.
#[must_use]
pub(crate) fn doc_hidden_violations(root: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    for dir in banned_dirs(root) {
        if !dir.is_dir() {
            violations.push(format!(
                "{}: the engine directory is missing, so its sources cannot be shown free of \
                 doc(hidden)",
                dir.display()
            ));
            continue;
        }
        let mut files = crate::tidy::rust_files(&dir);
        files.sort();
        for file in &files {
            match fs::read_to_string(file) {
                Ok(text) => violations.extend(source_violations(file, &text)),
                Err(error) => {
                    violations.push(format!("{}: unreadable source: {error}", file.display()));
                }
            }
        }
    }
    violations
}

/// The hidden attributes in one source file, labeled with `file`.
fn source_violations(file: &Path, text: &str) -> Vec<String> {
    let unparseable = |line: usize, error: &dyn std::fmt::Display| {
        vec![format!(
            "{}:{line}: unparseable source: required a file that parses as Rust, found {error}",
            file.display()
        )]
    };
    let tokens: TokenStream = match text.parse() {
        Ok(tokens) => tokens,
        Err(error) => return unparseable(error.span().start().line, &error),
    };
    if let Err(error) = syn::parse2::<syn::File>(tokens.clone()) {
        return unparseable(error.span().start().line, &error);
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut hidden = Vec::new();
    hidden_attribute_lines(tokens, &mut hidden);
    hidden
        .into_iter()
        .map(|line| {
            let found = line
                .checked_sub(1)
                .and_then(|index| lines.get(index))
                .map_or("", |text| text.trim());
            format!(
                "{}:{line}: {RULE}: required {REQUIRED}, found `{found}`",
                file.display()
            )
        })
        .collect()
}

/// Pushes the line of every attribute in `tokens` that hides what it sits
/// on, outer (`#[...]`) or inner (`#![...]`), at any depth.
fn hidden_attribute_lines(tokens: TokenStream, lines: &mut Vec<usize>) {
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            TokenTree::Group(group) => hidden_attribute_lines(group.stream(), lines),
            TokenTree::Punct(pound) if pound.as_char() == '#' => {
                if matches!(tokens.peek(), Some(TokenTree::Punct(bang)) if bang.as_char() == '!') {
                    tokens.next();
                }
                // The bracket group itself is left for the next turn of the
                // loop, which descends into it like any other group.
                if let Some(TokenTree::Group(content)) = tokens.peek()
                    && content.delimiter() == Delimiter::Bracket
                    && hides(content.stream())
                {
                    lines.push(pound.span().start().line);
                }
            }
            _ => {}
        }
    }
}

/// Whether an attribute's content, the tokens inside its brackets, hides
/// the item: `doc(...)` naming `hidden` among its settings, or
/// `cfg_attr(predicate, ...)` applying an attribute that does. The
/// predicate is skipped, so `cfg_attr(hidden, ...)` names a cfg, not a
/// doc setting.
fn hides(content: TokenStream) -> bool {
    let mut tokens = content.into_iter();
    let (Some(TokenTree::Ident(name)), Some(TokenTree::Group(args)), None) =
        (tokens.next(), tokens.next(), tokens.next())
    else {
        return false;
    };
    if args.delimiter() != Delimiter::Parenthesis {
        return false;
    }
    let settings = comma_separated(args.stream());
    if name == "doc" {
        settings.iter().any(
            |setting| matches!(setting.first(), Some(TokenTree::Ident(word)) if word == "hidden"),
        )
    } else if name == "cfg_attr" {
        settings
            .into_iter()
            .skip(1)
            .any(|attribute| hides(attribute.into_iter().collect()))
    } else {
        false
    }
}

/// Splits `tokens` at their top-level commas.
fn comma_separated(tokens: TokenStream) -> Vec<Vec<TokenTree>> {
    let mut parts = vec![Vec::new()];
    for token in tokens {
        match &token {
            TokenTree::Punct(comma) if comma.as_char() == ',' => parts.push(Vec::new()),
            _ => {
                if let Some(part) = parts.last_mut() {
                    part.push(token);
                }
            }
        }
    }
    parts
}

#[cfg(test)]
#[path = "doc_hidden-tests.rs"]
mod tests;
