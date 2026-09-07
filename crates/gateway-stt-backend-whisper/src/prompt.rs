//! Whisper conditioning prompt construction and fitting.

use gateway_whisper_ffi::WhisperContext;

const MAX_PROMPT_CHARS: usize = 800;
const MAX_PROMPT_TOKENS: usize = 224;
pub(crate) const GLOSSARY_TOKEN_BUDGET: usize = MAX_PROMPT_TOKENS / 2;

fn tail_chars(text: &str, max: usize) -> &str {
    let mut start = text.len().saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

pub(crate) fn sanitize_prompt(prompt: &str) -> String {
    let cleaned: String = prompt
        .chars()
        .filter(|&character| character != '\0')
        .collect();
    tail_chars(&cleaned, MAX_PROMPT_CHARS).to_owned()
}

fn glossary_prompt(vocabulary: &[String]) -> Option<String> {
    let terms: Vec<String> = vocabulary
        .iter()
        .map(|term| {
            term.trim()
                .chars()
                .filter(|&character| character != '\0')
                .collect::<String>()
        })
        .filter(|term| !term.is_empty())
        .collect();
    if terms.is_empty() {
        return None;
    }
    Some(format!("Glossary: {}.", terms.join(", ")))
}

fn token_count(context: &WhisperContext, text: &str) -> usize {
    context
        .tokenize(text, text.len().max(1))
        .map_or(usize::MAX, |tokens| tokens.len())
}

pub(crate) fn fit_glossary(
    context: &WhisperContext,
    vocabulary: &[String],
    budget: usize,
) -> Option<String> {
    let mut len = vocabulary.len();
    let mut fitted = glossary_prompt(vocabulary)?;
    while fitted.len() > MAX_PROMPT_CHARS || token_count(context, &fitted) > budget {
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

pub(crate) fn final_prompt(
    context: &WhisperContext,
    glossary: Option<&str>,
    transcript: &str,
) -> String {
    let Some(glossary) = glossary else {
        return sanitize_prompt(transcript);
    };
    let cleaned: String = transcript
        .chars()
        .filter(|&character| character != '\0')
        .collect();
    let char_budget = MAX_PROMPT_CHARS.saturating_sub(glossary.len() + 1);
    let mut tail = tail_chars(&cleaned, char_budget).trim_start();
    loop {
        if tail.is_empty() {
            return glossary.to_owned();
        }
        let combined = format!("{glossary} {tail}");
        if token_count(context, &combined) <= MAX_PROMPT_TOKENS {
            return combined;
        }
        tail = match tail.find(char::is_whitespace) {
            Some(index) => tail[index..].trim_start(),
            None => "",
        };
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use gateway_stt_engine::test_fixtures::native::require_fixture;
    use gateway_whisper_ffi::WhisperLibrary;

    use super::*;

    static NATIVE_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn native_fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    #[test]
    fn native_prompt_test_keeps_its_backend_fixture_root() {
        assert_eq!(
            native_fixture_root(),
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
        );
    }

    fn require_context() -> WhisperContext {
        let library_path = require_fixture(
            "PROMPTFORGE_WHISPER_LIBRARY",
            &native_fixture_root(),
            "whisper.dll",
        );
        let model_path = require_fixture(
            "PROMPTFORGE_WHISPER_MODEL",
            &native_fixture_root(),
            "ggml-tiny.en.bin",
        );
        let library = WhisperLibrary::load(&library_path).expect("packaged whisper runtime loads");
        WhisperContext::new(&library, &model_path).expect("whisper fixture model loads")
    }

    #[test]
    fn sanitize_prompt_strips_nulls_and_caps_length() {
        assert_eq!(sanitize_prompt("hello"), "hello");
        assert_eq!(sanitize_prompt("a\0b"), "ab");
        assert_eq!(
            sanitize_prompt(&"x".repeat(MAX_PROMPT_CHARS + 100)).len(),
            MAX_PROMPT_CHARS
        );
        let multibyte = sanitize_prompt(&"é".repeat(MAX_PROMPT_CHARS + 10));
        assert!(multibyte.len() <= MAX_PROMPT_CHARS);
        assert!(multibyte.chars().all(|character| character == 'é'));
    }

    #[test]
    fn glossary_prompt_rejects_empty_terms() {
        assert_eq!(glossary_prompt(&[]), None);
        assert_eq!(glossary_prompt(&[String::new()]), None);
        assert_eq!(glossary_prompt(&[" \0 ".to_owned()]), None);
    }

    #[test]
    fn glossary_prompt_cleans_and_formats_terms() {
        let vocabulary: Vec<String> = [" tokio ", "ax\0um", ""].map(str::to_owned).into();
        assert_eq!(
            glossary_prompt(&vocabulary),
            Some("Glossary: tokio, axum.".to_owned())
        );
    }

    #[test]
    #[ignore = "requires packaged whisper and model fixtures"]
    fn fit_glossary_enforces_character_and_token_boundaries() {
        let _guard = NATIVE_TEST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let context = require_context();

        let exact_char_limit = vec!["a".repeat(MAX_PROMPT_CHARS - "Glossary: .".len())];
        let exact = fit_glossary(&context, &exact_char_limit, usize::MAX)
            .expect("a glossary exactly at the character limit fits");
        assert_eq!(exact.len(), MAX_PROMPT_CHARS);
        let over_char_limit = vec!["a".repeat(MAX_PROMPT_CHARS - "Glossary: .".len() + 1)];
        assert_eq!(
            fit_glossary(&context, &over_char_limit, usize::MAX),
            None,
            "a glossary one character over the limit is rejected"
        );

        let vocabulary: Vec<String> = ["MCP", "GGUF", "Lua"].map(str::to_owned).into();
        let full = glossary_prompt(&vocabulary).expect("the vocabulary is usable");
        let exact_token_budget = token_count(&context, &full);
        assert_eq!(
            fit_glossary(&context, &vocabulary, exact_token_budget),
            Some(full.clone()),
            "a glossary exactly at the token limit fits"
        );
        let trimmed = fit_glossary(&context, &vocabulary, exact_token_budget - 1)
            .expect("the leading glossary terms still fit");
        assert_ne!(trimmed, full, "one token less forces truncation");
        assert!(token_count(&context, &trimmed) < exact_token_budget);
    }

    #[test]
    #[ignore = "requires packaged whisper and model fixtures"]
    fn final_prompt_enforces_combined_character_and_token_boundaries() {
        let _guard = NATIVE_TEST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let context = require_context();
        let glossary = "Glossary: MCP, GGUF, Lua.";

        let character_limited =
            final_prompt(&context, Some(glossary), &"a".repeat(MAX_PROMPT_CHARS * 2));
        assert_eq!(
            character_limited.len(),
            MAX_PROMPT_CHARS,
            "the combined prompt fills but never exceeds its character budget"
        );
        assert!(character_limited.starts_with(glossary));
        assert!(token_count(&context, &character_limited) <= MAX_PROMPT_TOKENS);

        let token_limited = final_prompt(&context, Some(glossary), &"x q z v j ".repeat(200));
        assert!(token_limited.starts_with(glossary));
        assert!(token_limited.len() <= MAX_PROMPT_CHARS);
        assert!(
            token_count(&context, &token_limited) <= MAX_PROMPT_TOKENS,
            "the combined prompt stays within whisper's token budget"
        );
        assert!(
            token_limited.len() < MAX_PROMPT_CHARS,
            "the token budget, not the character budget, limits this fixture"
        );
        assert!(
            token_limited.trim_end().ends_with("x q z v j"),
            "truncation retains the transcript tail"
        );
    }
}
