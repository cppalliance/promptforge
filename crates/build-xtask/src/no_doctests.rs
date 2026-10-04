//! The doctest ban, live: no doc comment in any workspace crate holds a
//! code block rustdoc would compile, and no doc attribute reads its text
//! from a file with `include_str!`. `cargo test --doc` never sees a
//! binary, a build script, or an integration test, so this check, not a
//! doctest run, is what shows none is left. Runs as part of
//! `cargo test -p build-xtask` and `cargo xtask tidy`.
//!
//! Each file must parse with `syn`; then every doc attribute in its token
//! tree is read, outer or inner, at any depth, including the tokens inside
//! a macro. An item's doc attributes are joined and unindented the way
//! rustdoc joins them, then parsed with `pulldown-cmark`. A block is
//! compiled when rustdoc would read it as Rust: an indented block, an
//! untagged fence, or a fence whose tags rustdoc reads as Rust. A `text`,
//! `json`, or `toml` fence is not.

use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};
use syn::punctuated::Punctuated;
use syn::{Expr, ExprLit, Lit, Meta, Token};

const RULE: &str = "doctest in a doc comment";
const REQUIRED: &str = "no compiled code block and no `include_str!` in doc comments: move the \
    example into the crate's tests";

/// One doc attribute: its text, the source line it starts on, and whether
/// it was written as a `///` or `//!` comment rather than as
/// `doc = "..."`, which rustdoc unindents differently.
struct Fragment {
    line: usize,
    text: String,
    sugared: bool,
}

/// What one file's token tree holds: each item's doc fragments, and the
/// line of every doc attribute whose value is an `include_str!`.
#[derive(Default)]
struct Found {
    docs: Vec<Vec<Fragment>>,
    includes: Vec<usize>,
}

/// Runs the ban over every `.rs` file in every workspace crate, the
/// crates whose manifest the shared walk could not read included. An
/// unreadable or unparseable file is reported, not skipped, and a scan
/// that found no source at all fails: a file that was never scanned cannot
/// be shown clean.
#[must_use]
pub(crate) fn no_doctests_violations(root: &Path) -> Vec<String> {
    let walk = crate::product::workspace_crates(root);
    let mut files: Vec<PathBuf> = walk
        .crates
        .iter()
        .map(|krate| &krate.dir)
        .chain(&walk.unread)
        .flat_map(|dir| crate::tidy::rust_files(&root.join(dir)))
        .collect();
    if files.is_empty() {
        return vec![format!(
            "{}: the doctest ban scanned nothing: no workspace crate holds a `.rs` file, so \
             none can be shown free of doctests",
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

/// The doctests in one source file, labeled with `file`.
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
    let mut found = Found::default();
    collect_docs(tokens, &lines, &mut found);
    let mut flagged = found.includes;
    flagged.extend(found.docs.iter().flat_map(|doc| compiled_block_lines(doc)));
    flagged.sort_unstable();
    flagged.dedup();
    flagged
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

/// Walks `tokens` at every depth, gathering each run of attributes into
/// the doc of the item it sits on. Rustdoc joins every doc attribute on an
/// item, whatever other attributes sit between them, so a run ends only at
/// a token that is not an attribute. Inner and outer docs in one run
/// belong to different items, so they are kept apart.
fn collect_docs(tokens: TokenStream, lines: &[&str], found: &mut Found) {
    let mut outer = Vec::new();
    let mut inner = Vec::new();
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            TokenTree::Punct(pound) if pound.as_char() == '#' => {
                let bang =
                    matches!(tokens.peek(), Some(TokenTree::Punct(bang)) if bang.as_char() == '!');
                if bang {
                    tokens.next();
                }
                if let Some(TokenTree::Group(content)) = tokens.peek()
                    && content.delimiter() == Delimiter::Bracket
                {
                    let start = pound.span().start();
                    // A doc comment's `#` carries the comment's own span.
                    let sugared = start
                        .line
                        .checked_sub(1)
                        .and_then(|index| lines.get(index))
                        .and_then(|text| text.chars().nth(start.column))
                        == Some('/');
                    if let Ok(meta) = syn::parse2::<Meta>(content.stream()) {
                        let docs = if bang { &mut inner } else { &mut outer };
                        read_meta(&meta, start.line, sugared, docs, found);
                    }
                    tokens.next();
                    continue;
                }
            }
            TokenTree::Group(group) => collect_docs(group.stream(), lines, found),
            _ => {}
        }
        flush(&mut outer, found);
        flush(&mut inner, found);
    }
    flush(&mut outer, found);
    flush(&mut inner, found);
}

/// Ends one item's doc: its fragments, if any, become one doc to classify.
fn flush(docs: &mut Vec<Fragment>, found: &mut Found) {
    if !docs.is_empty() {
        found.docs.push(std::mem::take(docs));
    }
}

/// Reads one attribute at `line`: `doc = "..."` adds its text to `docs`,
/// a doc value that is an `include_str!(...)` is recorded, and `cfg_attr(predicate, ...)`
/// reads each attribute it applies. Any other `doc` value, such as a
/// `concat!` or a macro metavariable, holds no block the scan can see.
fn read_meta(meta: &Meta, line: usize, sugared: bool, docs: &mut Vec<Fragment>, found: &mut Found) {
    match meta {
        Meta::NameValue(pair) if pair.path.is_ident("doc") => match &pair.value {
            Expr::Lit(ExprLit {
                lit: Lit::Str(text),
                ..
            }) => docs.push(Fragment {
                line,
                text: text.value(),
                sugared,
            }),
            Expr::Macro(call)
                if call
                    .mac
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "include_str") =>
            {
                found.includes.push(line);
            }
            _ => {}
        },
        Meta::List(list) if list.path.is_ident("cfg_attr") => {
            if let Ok(parts) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            {
                for applied in parts.iter().skip(1) {
                    read_meta(applied, line, false, docs, found);
                }
            }
        }
        _ => {}
    }
}

/// The source line each compiled block in one item's doc opens on.
fn compiled_block_lines(doc: &[Fragment]) -> Vec<usize> {
    let (text, starts) = rustdoc_text(doc);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION;
    let mut lines = Vec::new();
    for (event, range) in Parser::new_ext(&text, options).into_offset_iter() {
        let Event::Start(Tag::CodeBlock(kind)) = event else {
            continue;
        };
        let compiled = match kind {
            CodeBlockKind::Indented => true,
            CodeBlockKind::Fenced(info) => is_rust(&info),
        };
        let index = text
            .get(..range.start)
            .map_or(0, |before| before.matches('\n').count());
        if compiled && let Some(&line) = starts.get(index) {
            lines.push(line);
        }
    }
    lines
}

/// One item's doc as rustdoc reads it, beside the source line of each of
/// its lines. Rustdoc removes the indent every non-blank line shares; when
/// comment and `doc = "..."` fragments mix, the comments decide it and the
/// others count one more column, the space a comment's text starts with.
fn rustdoc_text(doc: &[Fragment]) -> (String, Vec<usize>) {
    let mixed = doc
        .windows(2)
        .any(|pair| pair[0].sugared != pair[1].sugared)
        && doc.iter().any(|fragment| fragment.sugared);
    let add = usize::from(mixed);
    let indent_of = |line: &str| line.chars().take_while(|c| matches!(c, ' ' | '\t')).count();
    let is_blank = |line: &str| line.chars().all(char::is_whitespace);
    let min_indent = doc
        .iter()
        .flat_map(|fragment| {
            let extra = if fragment.sugared { 0 } else { add };
            fragment
                .text
                .lines()
                .filter(|line| !is_blank(line))
                .map(move |line| indent_of(line) + extra)
        })
        .min()
        .unwrap_or(0);
    let mut text = String::new();
    let mut starts = Vec::new();
    for fragment in doc {
        if fragment.text.is_empty() {
            text.push('\n');
            starts.push(fragment.line);
            continue;
        }
        let indent = if fragment.sugared {
            min_indent
        } else {
            min_indent.saturating_sub(add)
        };
        for (offset, line) in fragment.text.lines().enumerate() {
            let kept = if is_blank(line) {
                line
            } else {
                line.get(indent..).unwrap_or(line)
            };
            text.push_str(kept);
            text.push('\n');
            starts.push(fragment.line + offset);
        }
    }
    (text, starts)
}

/// Whether rustdoc reads a fence's info string as Rust, following its
/// `LangString::parse` on stable: an untagged fence is Rust, and a tag
/// rustdoc does not know makes the block another language unless `rust`
/// appears anywhere or another Rust tag came before it.
fn is_rust(info: &str) -> bool {
    let mut seen_rust = false;
    let mut seen_other = false;
    for tag in info.split([',', ' ', '\t']).filter(|tag| !tag.is_empty()) {
        match tag {
            "rust" => seen_rust = true,
            "should_panic" | "no_run" | "ignore" => seen_rust = !seen_other,
            "test_harness" | "compile_fail" | "standalone_crate" => {
                seen_rust = !seen_other || seen_rust;
            }
            _ if tag.starts_with("edition") || tag.starts_with("ignore-") => {}
            _ => seen_other = true,
        }
    }
    !seen_other || seen_rust
}

#[cfg(test)]
#[path = "no_doctests-tests.rs"]
mod tests;
