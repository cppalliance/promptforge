//! Glob-pattern grammar and a bounded, recursion-free matcher.
//!
//! The store validates a caller-supplied glob against one grammar
//! (STORE-006) and then matches stored paths with a bounded iterative
//! dynamic program (STORE-005), so a hostile pattern cannot drive exponential
//! time or blow the stack.

/// The largest glob pattern, in bytes, the store will attempt to match.
///
/// The recursion-free matcher is linear, but an unbounded pattern is still a
/// cheap denial-of-service lever, so an over-long pattern is refused outright.
pub(crate) const MAX_GLOB_PATTERN_BYTES: usize = 1024;

/// One unit of a validated glob pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlobToken {
    /// A literal byte that must match exactly.
    Literal(u8),
    /// `*`: zero or more bytes, none of them `/` (stays within one segment).
    Star,
    /// `**` not bounded by a `/`: zero or more bytes of any kind.
    DoubleStar,
    /// `**/`: zero or more whole path segments (empty, or any run ending `/`).
    DoubleStarSlash,
}

/// Validates the glob grammar (STORE-006), rejecting unsupported forms.
///
/// The grammar is: literal bytes, `*` (within a segment), and `**` occupying a
/// whole segment (`**`, `**/...`, `.../**`, `.../**/...`). There is no escape
/// syntax, so a backslash is unsupported and runs of three or more `*` are
/// rejected rather than silently reinterpreted.
pub(crate) fn validate_glob_grammar(pattern: &str) -> Result<(), &'static str> {
    if pattern.contains('\\') {
        return Err("pattern does not support backslash escapes");
    }
    let bytes = pattern.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'*' {
            index += 1;
            continue;
        }
        let run_start = index;
        while index < bytes.len() && bytes[index] == b'*' {
            index += 1;
        }
        let run_len = index - run_start;
        if run_len > 2 {
            return Err("more than two consecutive '*' are not supported");
        }
        if run_len == 2 {
            let before_ok = run_start == 0 || bytes[run_start - 1] == b'/';
            let after_ok = index == bytes.len() || bytes[index] == b'/';
            if !before_ok || !after_ok {
                return Err("'**' must occupy a whole path segment");
            }
        }
    }
    Ok(())
}

/// Compiles an already-grammar-validated pattern into reusable [`GlobToken`]s.
///
/// STORE-020: callers that match one pattern against many paths compile once
/// and reuse the tokens via [`matches_tokens`], so the per-path tokenization
/// cost is not repeated for every key while a store lock is held.
pub(crate) fn compile_glob(pattern: &[u8]) -> Vec<GlobToken> {
    debug_assert!(
        std::str::from_utf8(pattern).is_ok_and(|text| validate_glob_grammar(text).is_ok()),
        "compile_glob requires validated grammar"
    );
    tokenize_glob(pattern)
}

/// Tokenizes an already-grammar-validated pattern into [`GlobToken`]s.
fn tokenize_glob(pattern: &[u8]) -> Vec<GlobToken> {
    let mut tokens = Vec::with_capacity(pattern.len());
    let mut index = 0;
    while index < pattern.len() {
        match pattern[index] {
            b'*' => {
                if pattern.get(index + 1) == Some(&b'*') {
                    if pattern.get(index + 2) == Some(&b'/') {
                        tokens.push(GlobToken::DoubleStarSlash);
                        index += 3;
                    } else {
                        tokens.push(GlobToken::DoubleStar);
                        index += 2;
                    }
                } else {
                    tokens.push(GlobToken::Star);
                    index += 1;
                }
            }
            byte => {
                tokens.push(GlobToken::Literal(byte));
                index += 1;
            }
        }
    }
    tokens
}

/// Matches `text` against a glob `pattern` where `*` stays within a segment and
/// `**` spans `/`.
///
/// STORE-005: bounded iterative dynamic programming over reachable text
/// positions, with no recursion and no suffix backtracking, so a hostile
/// pattern cannot drive exponential time or blow the stack. Runs in
/// `O(tokens * text_len)` time and `O(text_len)` space.
/// Compiles and matches in one shot. Retained as the parity reference for
/// [`matches_tokens`]; the store path uses [`compile_glob`] + [`matches_tokens`]
/// so it is only needed in tests.
#[cfg(test)]
pub(crate) fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    matches_tokens(&tokenize_glob(pattern), text)
}

/// Matches `text` against already-compiled glob `tokens` (see [`compile_glob`]).
///
/// Split from `glob_match` so one compiled pattern can be reused across many
/// paths without re-tokenizing per path (STORE-020). Same bounded iterative
/// dynamic program: `O(tokens * text_len)` time, `O(text_len)` space.
pub(crate) fn matches_tokens(tokens: &[GlobToken], text: &[u8]) -> bool {
    let len = text.len();
    // `reachable[j]` is true when some prefix of the pattern consumed so far
    // matches exactly `text[..j]`.
    let mut reachable = vec![false; len + 1];
    reachable[0] = true;
    let mut next = vec![false; len + 1];

    for &token in tokens {
        next.fill(false);
        match token {
            GlobToken::Literal(byte) => {
                for j in 0..len {
                    if reachable[j] && text[j] == byte {
                        next[j + 1] = true;
                    }
                }
            }
            GlobToken::Star => {
                // Zero or more non-`/` bytes: sweep left to right, carrying
                // reachability forward across each non-slash byte.
                let mut carry = false;
                for j in 0..=len {
                    let here = reachable[j] || carry;
                    next[j] = here;
                    carry = here && j < len && text[j] != b'/';
                }
            }
            GlobToken::DoubleStar => {
                // Zero or more bytes of any kind: once any position is
                // reachable, every later position is too.
                let mut seen = false;
                for j in 0..=len {
                    seen |= reachable[j];
                    next[j] = seen;
                }
            }
            GlobToken::DoubleStarSlash => {
                // Empty, or any run ending in `/` (whole path segments).
                let mut seen = false;
                for j in 0..=len {
                    let mut here = reachable[j];
                    if seen && j > 0 && text[j - 1] == b'/' {
                        here = true;
                    }
                    next[j] = here;
                    if reachable[j] {
                        seen = true;
                    }
                }
            }
        }
        std::mem::swap(&mut reachable, &mut next);
    }
    reachable[len]
}

#[cfg(test)]
mod tests {
    use super::{compile_glob, glob_match, matches_tokens};

    #[test]
    fn compiled_and_one_shot_match_expected_results_across_many_paths() {
        // STORE-020: a pattern compiled once and reused across many keys must
        // produce the pinned result for every key. The one-shot matcher is
        // checked against the same independent expectations.
        let paths = [
            "a.txt",
            "src/a.rs",
            "src/b.rs",
            "src/deep/c.rs",
            "src/deep/deeper/d.rs",
            "notes/today.md",
        ];
        for (pattern, expected) in [
            ("*.txt", [true, false, false, false, false, false]),
            ("src/*.rs", [false, true, true, false, false, false]),
            ("src/**/*.rs", [false, true, true, true, true, false]),
            ("**/*.md", [false, false, false, false, false, true]),
            ("src/**", [false, true, true, true, true, false]),
            ("no*match", [false, false, false, false, false, false]),
        ] {
            let tokens = compile_glob(pattern.as_bytes());
            for (path, expected) in paths.iter().zip(expected) {
                assert_eq!(
                    matches_tokens(&tokens, path.as_bytes()),
                    expected,
                    "compiled pattern {pattern:?} produced the wrong result for {path:?}",
                );
                assert_eq!(
                    glob_match(pattern.as_bytes(), path.as_bytes()),
                    expected,
                    "one-shot pattern {pattern:?} produced the wrong result for {path:?}",
                );
            }
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "compile_glob requires validated grammar")]
    fn compile_glob_rejects_unvalidated_direct_input_in_debug_builds() {
        let _ = compile_glob(b"bad/***/pattern");
    }
}
