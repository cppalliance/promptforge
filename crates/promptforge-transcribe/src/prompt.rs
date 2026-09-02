//! Whisper conditioning prompts: glossary fitting and transcript tails.

use whisper_ffi::WhisperContext;

use crate::{MAX_PROMPT_CHARS, MAX_PROMPT_TOKENS};

/// The trailing `max` bytes of `text`, cut at a char boundary.
fn tail_chars(text: &str, max: usize) -> &str {
    let mut start = text.len().saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// The trailing `MAX_PROMPT_CHARS` chars of `prompt` with null bytes
/// stripped: whisper's prompt buffer is bounded, and `set_initial_prompt`
/// panics on null bytes, which a model transcript could in principle
/// contain.
pub(super) fn sanitize_prompt(prompt: &str) -> String {
    let cleaned: String = prompt.chars().filter(|&c| c != '\0').collect();
    tail_chars(&cleaned, MAX_PROMPT_CHARS).to_string()
}

/// Formats `vocabulary` as a whisper conditioning prompt in glossary form:
/// `Glossary: a, b, c.` Terms are trimmed and null bytes stripped (whisper
/// tokenization rejects them); a vocabulary with no usable terms yields
/// `None`. The glossary format is a soft probabilistic bias, and measurably
/// outperforms a raw keyword list.
pub(crate) fn glossary_prompt(vocabulary: &[String]) -> Option<String> {
    let terms: Vec<String> = vocabulary
        .iter()
        .map(|term| {
            term.trim()
                .chars()
                .filter(|&c| c != '\0')
                .collect::<String>()
        })
        .filter(|term| !term.is_empty())
        .collect();
    if terms.is_empty() {
        return None;
    }
    Some(format!("Glossary: {}.", terms.join(", ")))
}

/// Token count of `text` under the model's tokenizer, or `usize::MAX`
/// when tokenization fails (for example on null bytes, though callers
/// strip those first).
///
/// Tokenizing with one slot per byte - an upper bound on the token count -
/// means the native buffer always has enough capacity.
fn token_count(ctx: &WhisperContext, text: &str) -> usize {
    ctx.tokenize(text, text.len().max(1))
        .map_or(usize::MAX, |tokens| tokens.len())
}

/// Fits the glossary prompt for `vocabulary` within `budget` whisper tokens
/// (and the prompt char cap), dropping whole terms from the end until it
/// fits. Returns `None` when the vocabulary has no usable terms or no term
/// fits, and logs a warning when terms were dropped.
pub(super) fn fit_glossary(
    ctx: &WhisperContext,
    vocabulary: &[String],
    budget: usize,
) -> Option<String> {
    let mut len = vocabulary.len();
    let mut fitted = glossary_prompt(vocabulary)?;
    while fitted.len() > MAX_PROMPT_CHARS || token_count(ctx, &fitted) > budget {
        len -= 1;
        if len == 0 {
            tracing::warn!("no voice vocabulary term fits the prompt budget");
            return None;
        }
        fitted = glossary_prompt(&vocabulary[..len])?;
    }
    if len < vocabulary.len() {
        tracing::warn!(
            kept = len,
            dropped = vocabulary.len() - len,
            "voice vocabulary truncated to fit whisper's prompt budget"
        );
    }
    Some(fitted)
}

/// Builds the final pass's conditioning prompt: the fitted glossary
/// followed by as much of the accumulated transcript's tail as fits within
/// the char cap and whisper's 224-token prompt budget. The transcript trims
/// from the front (its tail carries the continuity); the glossary is never
/// trimmed here - it was fitted to its own budget at load.
pub(super) fn final_prompt(
    ctx: &WhisperContext,
    glossary: Option<&str>,
    transcript: &str,
) -> String {
    let Some(glossary) = glossary else {
        return sanitize_prompt(transcript);
    };
    let cleaned: String = transcript.chars().filter(|&c| c != '\0').collect();
    let char_budget = MAX_PROMPT_CHARS.saturating_sub(glossary.len() + 1);
    let mut tail = tail_chars(&cleaned, char_budget).trim_start();
    loop {
        if tail.is_empty() {
            return glossary.to_string();
        }
        let combined = format!("{glossary} {tail}");
        if token_count(ctx, &combined) <= MAX_PROMPT_TOKENS {
            return combined;
        }
        // Drop the tail's first word and retry; a single oversized word is
        // dropped whole, which ends the loop on the next iteration.
        tail = match tail.find(char::is_whitespace) {
            Some(index) => tail[index..].trim_start(),
            None => "",
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{GLOSSARY_TOKEN_BUDGET, fixtures};

    #[test]
    fn sanitize_prompt_strips_nulls_and_caps_length() {
        assert_eq!(sanitize_prompt("hello"), "hello");
        assert_eq!(sanitize_prompt("a\0b"), "ab");
        let long = "x".repeat(MAX_PROMPT_CHARS + 100);
        assert_eq!(sanitize_prompt(&long).len(), MAX_PROMPT_CHARS);
        // Multibyte input is capped at a char boundary, never mid-codepoint.
        let multibyte = "é".repeat(MAX_PROMPT_CHARS + 10);
        let capped = sanitize_prompt(&multibyte);
        assert!(capped.len() <= MAX_PROMPT_CHARS);
        assert!(capped.chars().all(|c| c == 'é'));
    }

    #[test]
    fn glossary_prompt_is_none_without_usable_terms() {
        assert_eq!(glossary_prompt(&[]), None);
        assert_eq!(glossary_prompt(&[String::new()]), None);
        assert_eq!(glossary_prompt(&["  ".to_string()]), None);
        assert_eq!(glossary_prompt(&["\0".to_string()]), None);
    }

    #[test]
    fn glossary_prompt_formats_a_glossary() {
        let vocabulary: Vec<String> = ["MCP", "GGUF", "Lua"].map(str::to_string).into();
        assert_eq!(
            glossary_prompt(&vocabulary),
            Some("Glossary: MCP, GGUF, Lua.".to_string())
        );
    }

    #[test]
    fn glossary_prompt_cleans_terms() {
        let vocabulary: Vec<String> = [" tokio ", "ax\0um", ""].map(str::to_string).into();
        assert_eq!(
            glossary_prompt(&vocabulary),
            Some("Glossary: tokio, axum.".to_string())
        );
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn fit_glossary_keeps_a_vocabulary_that_fits() {
        let (_library, ctx) = fixtures::require_context();
        let vocabulary: Vec<String> = ["MCP", "GGUF", "Lua"].map(str::to_string).into();
        let fitted =
            fit_glossary(&ctx, &vocabulary, GLOSSARY_TOKEN_BUDGET).expect("a short glossary fits");
        assert_eq!(fitted, "Glossary: MCP, GGUF, Lua.");
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn fit_glossary_drops_terms_from_the_end_to_fit() {
        let (_library, ctx) = fixtures::require_context();
        let mut vocabulary: Vec<String> = ["MCP".to_string()].into();
        for index in 0..200 {
            vocabulary.push(format!("internationalization{index}"));
        }
        let fitted = fit_glossary(&ctx, &vocabulary, GLOSSARY_TOKEN_BUDGET)
            .expect("the leading terms still fit");
        assert!(
            fitted.starts_with("Glossary: MCP, "),
            "truncation keeps the leading terms: {fitted:?}"
        );
        assert!(
            fitted.len() <= MAX_PROMPT_CHARS,
            "the fitted glossary respects the char cap"
        );
        assert!(
            token_count(&ctx, &fitted) <= GLOSSARY_TOKEN_BUDGET,
            "the fitted glossary tokenizes within its budget: {fitted:?}"
        );
        let kept = fitted.matches(", ").count();
        assert!(
            kept < vocabulary.len(),
            "terms were dropped to fit: {kept} of {}",
            vocabulary.len()
        );
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn final_prompt_without_a_glossary_matches_sanitize() {
        let (_library, ctx) = fixtures::require_context();
        let transcript = "the quick brown fox ".repeat(100);
        assert_eq!(
            final_prompt(&ctx, None, &transcript),
            sanitize_prompt(&transcript)
        );
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn final_prompt_prepends_the_glossary_and_caps_tokens() {
        let (_library, ctx) = fixtures::require_context();
        let glossary = "Glossary: MCP, GGUF, Lua.";
        assert_eq!(
            final_prompt(&ctx, Some(glossary), ""),
            glossary,
            "an empty transcript leaves the glossary alone"
        );
        let transcript = "the quick brown fox jumps over the lazy dog ".repeat(100);
        let prompt = final_prompt(&ctx, Some(glossary), &transcript);
        assert!(
            prompt.starts_with(glossary),
            "the glossary leads the prompt: {prompt:?}"
        );
        assert!(
            prompt.len() <= MAX_PROMPT_CHARS,
            "the combined prompt respects the char cap"
        );
        assert!(
            token_count(&ctx, &prompt) <= MAX_PROMPT_TOKENS,
            "the combined prompt tokenizes within whisper's budget"
        );
        assert!(
            prompt.contains("lazy dog"),
            "the transcript's tail survives the trim: {prompt:?}"
        );
    }
}
