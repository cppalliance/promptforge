//! The one prompt-local alias grammar, shared by the `tools` and `models`
//! Engine globals.

use crate::error::{Error, Result};

/// A prompt-local alias that passed the supported wire grammar.
///
/// Local tool aliases and model labels share the one rule. An alias never
/// contains `/`, so it never reads as a tool id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Alias(String);

impl Alias {
    /// Validates `alias` against the wire grammar and wraps it.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when `alias` is empty, exceeds 64 bytes, starts
    /// with a non-letter, or contains a character other than a letter,
    /// digit, `_`, or `-` after its first byte.
    pub(crate) fn parse(alias: String) -> Result<Alias> {
        let bytes = alias.as_bytes();
        let valid = (1..=64).contains(&bytes.len())
            && bytes[0].is_ascii_alphabetic()
            && bytes[1..]
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        if valid {
            Ok(Alias(alias))
        } else {
            Err(Error::Lua(format!(
                "invalid alias {alias:?}: expected [A-Za-z][A-Za-z0-9_-]{{0,63}}"
            )))
        }
    }

    /// Returns the alias text.
    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<Alias> for String {
    fn from(alias: Alias) -> String {
        alias.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_alias_grammar_accepts_letters_digits_underscores_and_dashes() {
        for alias in [
            "a",
            "A",
            "search",
            "web_fetch",
            "web-fetch2",
            &"z".repeat(64),
        ] {
            let parsed = Alias::parse(alias.to_owned()).expect("the alias validates");
            assert_eq!(parsed.as_str(), alias, "{alias:?} keeps its text");
        }
    }

    #[test]
    fn the_alias_grammar_rejects_every_other_shape() {
        for alias in [
            "",
            "1search",
            "_search",
            "-search",
            "web fetch",
            "web.fetch",
            "web/fetch",
            &"a".repeat(65),
        ] {
            assert!(
                Alias::parse(alias.to_owned()).is_err(),
                "{alias:?} must not validate"
            );
        }
        let error = Alias::parse("1search".to_owned()).expect_err("the message names the grammar");
        assert!(
            error
                .to_string()
                .contains("invalid alias \"1search\": expected [A-Za-z][A-Za-z0-9_-]{0,63}"),
            "unexpected message: {error}"
        );
    }
}
