//! Retired-symbol scan: a retired engine symbol may not reappear in live
//! engine source.
//!
//! The sans-I/O engine plan retires a set of identifiers (`Observer`,
//! `GatewaySource`, `LuaFanoutResult`, ...). Once they are gone, this scan
//! keeps them gone: it walks a source root, strips comments and string
//! literals (a mention in prose or a message is not a reappearance), drops
//! every item under `#[cfg(test)]` (inline modules, module files named by
//! `mod name;` or `#[path = "..."]` under any visibility, and any other
//! test-only item), skips `tests/` directories and any path component
//! containing `test_support`, and reports whole-identifier matches against
//! the seed list.
//!
//! A `.rs` file the scan cannot read is skipped rather than reported. The
//! skip hides nothing: a module the compiler cannot read fails the build
//! that runs beside this guard, and a file no `mod` declaration names is
//! not compiled and so is not live code either way.
//!
//! The lexer is a masking pass, not a parser: it replaces stripped text
//! with spaces while keeping newlines, so byte offsets and line numbers
//! survive and the `#[cfg(test)]` pass can count braces without being
//! fooled by braces inside literals or comments.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// One identifier match in live source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Hit {
    /// The source file holding the identifier.
    pub(crate) file: PathBuf,
    /// The one-based line the identifier sits on.
    pub(crate) line: usize,
    /// The retired symbol that matched.
    pub(crate) symbol: String,
}

impl fmt::Display for Hit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: retired symbol {} reappears in live source",
            self.file.display(),
            self.line,
            self.symbol
        )
    }
}

/// Scan every live `.rs` file under `source_root` for the `seeds`, sorted
/// by file then line. An absent or unreadable root yields no hits, and an
/// unreadable file is skipped (see the module docs for why that is safe).
#[must_use]
pub(crate) fn retired_symbols(source_root: &Path, seeds: &[&str]) -> Vec<Hit> {
    let mut files = Vec::new();
    collect_sources(source_root, &mut files);
    let mut masked = Vec::new();
    let mut excluded = BTreeSet::new();
    for file in files {
        // Unreadable or non-UTF-8: rustc would reject it too if it were a
        // compiled module, so the build fails beside us; otherwise it is dead.
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        let mut code: Vec<char> = text.chars().collect();
        mask_comments_and_literals(&mut code);
        let original: Vec<char> = text.chars().collect();
        for path in remove_cfg_test_items(&mut code, &original, &file) {
            excluded.insert(normalize(&path));
        }
        masked.push((file, code));
    }
    let mut hits = Vec::new();
    for (file, code) in masked {
        if excluded.contains(&normalize(&file)) {
            continue;
        }
        let text: String = code.into_iter().collect();
        for (index, line) in text.lines().enumerate() {
            for token in identifiers(line) {
                if seeds.contains(&token) {
                    hits.push(Hit {
                        file: file.clone(),
                        line: index + 1,
                        symbol: token.to_owned(),
                    });
                }
            }
        }
    }
    hits.sort();
    hits
}

/// Whether a path component marks test-support code.
fn is_test_support(component: &str) -> bool {
    component.contains("test_support") || component.contains("test-support")
}

/// Every `.rs` file under `dir`, skipping `tests/` and `target/`
/// directories and any component naming test support.
fn collect_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if is_test_support(&name) {
            continue;
        }
        if path.is_dir() {
            if name != "tests" && name != "target" {
                collect_sources(&path, files);
            }
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

/// A path with its separators and `.` components normalized, so a module
/// path built from a `#[path = "a/b.rs"]` attribute compares equal to the
/// same file found by the directory walk.
fn normalize(path: &Path) -> PathBuf {
    path.components().collect()
}

/// Whole identifier-like tokens in one line: maximal runs of word
/// characters. Numeric-leading runs never match a seed, so they are
/// harmless.
fn identifiers(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|token| !token.is_empty())
}

/// Replace every character in `start..end` with a space, keeping newlines
/// so line numbers survive.
fn blank(code: &mut [char], start: usize, end: usize) {
    let end = end.min(code.len());
    for c in &mut code[start..end] {
        if *c != '\n' {
            *c = ' ';
        }
    }
}

/// Mask comments (line, doc, and nested block) and literals (strings, raw
/// strings, byte and C strings, chars) in place.
fn mask_comments_and_literals(code: &mut [char]) {
    let mut i = 0;
    while i < code.len() {
        let c = code[i];
        let next = code.get(i + 1).copied();
        let end = if c == '/' && next == Some('/') {
            line_end(code, i)
        } else if c == '/' && next == Some('*') {
            block_comment_end(code, i)
        } else if c == '"' {
            string_end(code, i + 1)
        } else if c == '\'' {
            let Some(end) = char_literal_end(code, i) else {
                i += 1;
                continue;
            };
            end
        } else if let Some((quote, hashes)) = raw_string_start(code, i) {
            raw_string_end(code, quote + 1, hashes)
        } else {
            i += 1;
            continue;
        };
        blank(code, i, end);
        i = end;
    }
}

/// The index just past the current line's newline (or the end of input).
fn line_end(code: &[char], from: usize) -> usize {
    code[from..]
        .iter()
        .position(|&c| c == '\n')
        .map_or(code.len(), |offset| from + offset)
}

/// The index just past the block comment opening at `from`, honoring
/// nesting.
fn block_comment_end(code: &[char], from: usize) -> usize {
    let mut depth = 0usize;
    let mut i = from;
    while i + 1 < code.len() {
        if code[i] == '/' && code[i + 1] == '*' {
            depth += 1;
            i += 2;
        } else if code[i] == '*' && code[i + 1] == '/' {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += 1;
        }
    }
    code.len()
}

/// The index just past the closing quote of a string whose body starts at
/// `from`, honoring backslash escapes.
fn string_end(code: &[char], from: usize) -> usize {
    let mut i = from;
    while i < code.len() {
        match code[i] {
            '\\' => i += 2,
            '"' => return i + 1,
            _ => i += 1,
        }
    }
    code.len()
}

/// When `at` opens a char literal, the index just past its closing quote;
/// `None` when it is a lifetime or label.
fn char_literal_end(code: &[char], at: usize) -> Option<usize> {
    match code.get(at + 1)? {
        '\\' => {
            // An escape: `'\n'`, `'\''`, `'\x7f'`, `'\u{1F600}'`. The
            // closing quote is the first quote after the escaped character.
            let mut i = at + 3;
            while i < code.len() && i < at + 12 {
                if code[i] == '\'' {
                    return Some(i + 1);
                }
                i += 1;
            }
            None
        }
        _ if code.get(at + 2) == Some(&'\'') => Some(at + 3),
        _ => None,
    }
}

/// When `at` opens a raw string (`r"`, `r#"`, `br"`, `cr##"`, ...), the
/// index of its opening quote and the number of hashes.
fn raw_string_start(code: &[char], at: usize) -> Option<(usize, usize)> {
    let preceded_by_word = at > 0 && (code[at - 1].is_alphanumeric() || code[at - 1] == '_');
    if preceded_by_word {
        return None;
    }
    let mut i = at;
    if matches!(code[i], 'b' | 'c') {
        i += 1;
    }
    if code.get(i) != Some(&'r') {
        return None;
    }
    i += 1;
    let hashes = code[i..].iter().take_while(|&&c| c == '#').count();
    i += hashes;
    (code.get(i) == Some(&'"')).then_some((i, hashes))
}

/// The index just past the closing `"###` of a raw string whose body
/// starts at `from`.
fn raw_string_end(code: &[char], from: usize, hashes: usize) -> usize {
    let mut i = from;
    while i < code.len() {
        if code[i] == '"'
            && code[i + 1..]
                .iter()
                .take(hashes)
                .filter(|&&c| c == '#')
                .count()
                == hashes
        {
            return i + 1 + hashes;
        }
        i += 1;
    }
    code.len()
}

/// The attribute that opts an item out of the live scan.
const CFG_TEST: &[char] = &['#', '[', 'c', 'f', 'g', '(', 't', 'e', 's', 't', ')', ']'];

/// Blank every item under `#[cfg(test)]` in the masked `code`, and return
/// the module files such items name (`mod name;`, with or without a
/// `#[path]`), resolved against `file`. `original` is the unmasked text at
/// the same indices, read for the `#[path]` value.
fn remove_cfg_test_items(code: &mut [char], original: &[char], file: &Path) -> Vec<PathBuf> {
    let mut modules = Vec::new();
    let mut search = 0;
    while let Some(offset) = code[search..]
        .windows(CFG_TEST.len())
        .position(|window| window == CFG_TEST)
    {
        let start = search + offset;
        let mut i = start + CFG_TEST.len();
        let mut path_attr = None;
        // Further attributes on the same item.
        loop {
            i = skip_whitespace(code, i);
            if code.get(i) == Some(&'#') && code.get(i + 1) == Some(&'[') {
                let close = balanced_end(code, i + 1, '[', ']');
                if let Some(path) = path_attribute(original, i, close) {
                    path_attr = Some(path);
                }
                i = close;
            } else {
                break;
            }
        }
        let end = match external_module(code, i) {
            Some((name, semicolon)) => {
                modules.extend(module_files(file, &name, path_attr.as_deref()));
                semicolon
            }
            None => item_end(code, i),
        };
        blank(code, start, end);
        search = end;
    }
    modules
}

fn skip_whitespace(code: &[char], mut i: usize) -> usize {
    while code.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    i
}

/// The index just past the bracket closing the one opened at `open`.
fn balanced_end(code: &[char], open: usize, opener: char, closer: char) -> usize {
    let mut depth = 0usize;
    for (i, &c) in code.iter().enumerate().skip(open) {
        if c == opener {
            depth += 1;
        } else if c == closer {
            depth -= 1;
            if depth == 0 {
                return i + 1;
            }
        }
    }
    code.len()
}

/// The string value of a `#[path = "..."]` attribute spanning
/// `start..end` of the unmasked text, if that is what the attribute is.
fn path_attribute(original: &[char], start: usize, end: usize) -> Option<String> {
    let text: String = original[start..end.min(original.len())].iter().collect();
    let body = text.strip_prefix("#[")?.strip_suffix(']')?.trim();
    let value = body
        .strip_prefix("path")?
        .trim_start()
        .strip_prefix('=')?
        .trim();
    let value = value.strip_prefix('"')?.strip_suffix('"')?;
    Some(value.to_owned())
}

/// The index just past an optional `pub` or `pub(...)` visibility
/// qualifier at `i` and the whitespace after it; `i` itself when the item
/// carries none.
fn skip_visibility(code: &[char], i: usize) -> usize {
    let keyword: String = code
        .get(i..i + 3)
        .map(|w| w.iter().collect())
        .unwrap_or_default();
    if keyword != "pub" {
        return i;
    }
    let after = i + 3;
    if !code
        .get(after)
        .is_some_and(|c| c.is_whitespace() || *c == '(')
    {
        return i;
    }
    let mut j = skip_whitespace(code, after);
    if code.get(j) == Some(&'(') {
        j = skip_whitespace(code, balanced_end(code, j, '(', ')'));
    }
    j
}

/// When the item at `i` is `mod name;`, with or without a visibility
/// qualifier (`pub mod tests;`, `pub(crate) mod fixtures;`), its name and
/// the index just past the semicolon.
fn external_module(code: &[char], i: usize) -> Option<(String, usize)> {
    let i = skip_visibility(code, i);
    let keyword: String = code.get(i..i + 3)?.iter().collect();
    if keyword != "mod" || !code.get(i + 3)?.is_whitespace() {
        return None;
    }
    let name_start = skip_whitespace(code, i + 3);
    let name_end = name_start
        + code[name_start..]
            .iter()
            .take_while(|c| c.is_alphanumeric() || **c == '_')
            .count();
    let after = skip_whitespace(code, name_end);
    (code.get(after) == Some(&';'))
        .then(|| (code[name_start..name_end].iter().collect(), after + 1))
}

/// The index just past the end of the item starting at `i`: the first `;`
/// outside any bracket, or the `}` closing the item's first top-level
/// brace block.
fn item_end(code: &[char], i: usize) -> usize {
    let mut depth = 0usize;
    for (index, &c) in code.iter().enumerate().skip(i) {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return index + 1;
                }
            }
            ';' if depth == 0 => return index + 1,
            _ => {}
        }
    }
    code.len()
}

/// The files `mod name;` in `file` may resolve to: the `#[path]` target
/// relative to the file's directory, or `name.rs` and `name/mod.rs` under
/// the file's module directory.
fn module_files(file: &Path, name: &str, path_attr: Option<&str>) -> Vec<PathBuf> {
    let dir = file.parent().unwrap_or(Path::new(""));
    if let Some(rel) = path_attr {
        return vec![dir.join(rel)];
    }
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let base = if matches!(stem, "mod" | "lib" | "main") {
        dir.to_path_buf()
    } else {
        dir.join(stem)
    };
    vec![
        base.join(format!("{name}.rs")),
        base.join(name).join("mod.rs"),
    ]
}

#[cfg(test)]
#[path = "retired_symbols-tests.rs"]
mod tests;
