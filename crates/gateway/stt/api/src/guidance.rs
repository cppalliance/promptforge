//! Decode guidance terms taken from a client's transcription prompt.

/// The most bytes whisper's glossary can hold, which is the 800-character
/// prompt limit in the whisper backend (`MAX_PROMPT_CHARS`).
const GLOSSARY_BYTE_LIMIT: usize = 800;

/// The separator the glossary puts between terms.
const TERM_SEPARATOR: &str = ", ";

/// Splits a client prompt into the terms whisper is biased toward.
///
/// Terms are comma-separated. Each is trimmed, and empty ones are dropped.
///
/// The backend keeps the longest leading run of terms that fits its glossary
/// and discards the rest, one term per rebuild, so every term it would
/// discard only adds cost. This stops once the terms alone would overflow the
/// glossary, which bounds both the term count and the bytes copied on every
/// decode however large the prompt is. The configured vocabulary comes first
/// and only shrinks the room left, so no term that could fit is dropped.
pub(crate) fn prompt_terms(prompt: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut joined_len = 0;
    for term in prompt
        .split(',')
        .map(str::trim)
        .filter(|term| !term.is_empty())
    {
        joined_len += term.len();
        if !terms.is_empty() {
            joined_len += TERM_SEPARATOR.len();
        }
        if joined_len > GLOSSARY_BYTE_LIMIT {
            break;
        }
        terms.push(term.to_owned());
    }
    terms
}

#[cfg(test)]
#[path = "guidance-tests.rs"]
mod tests;
