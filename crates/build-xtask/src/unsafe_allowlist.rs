//! The unsafe allowlist: the workspace lint table denies `unsafe_code`,
//! and only the four crates that own an unsafe boundary may relax it. Any
//! other `.rs` file that expects or allows `unsafe_code`, directly or
//! through `cfg_attr` at any depth, is a violation. Runs as part of
//! `cargo test -p build-xtask` and `cargo xtask tidy`.
//!
//! Each file is tokenized, and every attribute in its token tree is read,
//! outer or inner, at any depth, including the tokens inside a macro. A
//! string literal is one token, so text that only spells the attribute,
//! such as this crate's own test fixtures, is never read as one.

use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use syn::punctuated::Punctuated;
use syn::{Meta, Token};

/// The crate directories, relative to the workspace root, that own an
/// unsafe boundary and may relax `unsafe_code` in the module that holds it.
const OWNED: [&str; 4] = [
    "crates/gateway/app",
    "crates/gateway/stt/whisper-ffi",
    "crates/gateway-api-discovery",
    "crates/workshop/desktop",
];

/// Runs the allowlist over every `.rs` file in every workspace crate
/// outside [`OWNED`], the crates whose manifest the shared walk could not
/// read included. An unreadable or unparseable file is reported, not
/// skipped, and a scan that found no source at all fails: a file that was
/// never scanned cannot be shown clean.
#[must_use]
pub(crate) fn unsafe_allowlist_violations(root: &Path) -> Vec<String> {
    let walk = crate::product::workspace_crates(root);
    let mut files: Vec<PathBuf> = walk
        .crates
        .iter()
        .map(|krate| &krate.dir)
        .chain(&walk.unread)
        .filter(|dir| !is_owned(dir))
        .flat_map(|dir| crate::tidy::rust_files(&root.join(dir)))
        .collect();
    if files.is_empty() {
        return vec![format!(
            "{}: the unsafe allowlist scanned nothing: no workspace crate outside the owned \
             unsafe boundaries holds a `.rs` file, so none can be shown clean",
            root.join("crates").display()
        )];
    }
    files.sort();
    let mut violations = Vec::new();
    for file in &files {
        match fs::read_to_string(file) {
            Ok(text) => violations.extend(source_violations(file, &text)),
            Err(error) => {
                violations.push(format!("{}: unreadable source: {error}", file.display()));
            }
        }
    }
    violations
}

/// Whether `dir`, relative to the workspace root, is one of [`OWNED`].
fn is_owned(dir: &Path) -> bool {
    let parts: Vec<_> = dir
        .components()
        .map(std::path::Component::as_os_str)
        .collect();
    OWNED.iter().any(|owned| {
        owned
            .split('/')
            .map(std::ffi::OsStr::new)
            .eq(parts.iter().copied())
    })
}

/// The relaxations of `unsafe_code` in one source file, labeled with `file`.
fn source_violations(file: &Path, text: &str) -> Vec<String> {
    let tokens: TokenStream = match text.parse() {
        Ok(tokens) => tokens,
        Err(error) => {
            return vec![format!(
                "{}:{}: unparseable source: required a file that tokenizes as Rust, found {error}",
                file.display(),
                error.span().start().line
            )];
        }
    };
    let mut lines = Vec::new();
    collect_relaxations(tokens, &mut lines);
    lines
        .into_iter()
        .map(|line| {
            format!(
                "{}:{line}: relaxes `unsafe_code` outside the owned unsafe boundaries: required \
                 no `expect` or `allow` of `unsafe_code` outside {}, found one",
                file.display(),
                OWNED.join(", ")
            )
        })
        .collect()
}

/// Walks `tokens` at every depth and records the line of each attribute
/// that expects or allows `unsafe_code`.
fn collect_relaxations(tokens: TokenStream, lines: &mut Vec<usize>) {
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            TokenTree::Punct(pound) if pound.as_char() == '#' => {
                if matches!(tokens.peek(), Some(TokenTree::Punct(bang)) if bang.as_char() == '!') {
                    tokens.next();
                }
                if let Some(TokenTree::Group(content)) = tokens.peek()
                    && content.delimiter() == Delimiter::Bracket
                {
                    if syn::parse2::<Meta>(content.stream()).is_ok_and(|meta| relaxes(&meta)) {
                        lines.push(pound.span().start().line);
                    }
                    tokens.next();
                }
            }
            TokenTree::Group(group) => collect_relaxations(group.stream(), lines),
            _ => {}
        }
    }
}

/// Whether one attribute expects or allows `unsafe_code`, directly or
/// through each attribute a `cfg_attr` applies.
fn relaxes(meta: &Meta) -> bool {
    let Meta::List(list) = meta else {
        return false;
    };
    let Ok(parts) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) else {
        return false;
    };
    if list.path.is_ident("cfg_attr") {
        parts.iter().skip(1).any(relaxes)
    } else if list.path.is_ident("expect") || list.path.is_ident("allow") {
        parts
            .iter()
            .any(|part| matches!(part, Meta::Path(path) if path.is_ident("unsafe_code")))
    } else {
        false
    }
}

#[cfg(test)]
#[path = "unsafe_allowlist-tests.rs"]
mod tests;
