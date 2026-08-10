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
enum GlobToken {
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
pub(crate) fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let tokens = tokenize_glob(pattern);
    let len = text.len();
    // `reachable[j]` is true when some prefix of the pattern consumed so far
    // matches exactly `text[..j]`.
    let mut reachable = vec![false; len + 1];
    reachable[0] = true;
    let mut next = vec![false; len + 1];

    for token in tokens {
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
