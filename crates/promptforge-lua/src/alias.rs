//! The one prompt-local alias grammar, shared by the `tools` and `models`
//! host tables.

use crate::{Error, Result};

/// Validates a prompt-local alias against the supported wire grammar.
///
/// Aliases are the only names the model sees; tool slots and model roles
/// share the one rule.
///
/// # Errors
/// Returns [`Error::Lua`] when `alias` is empty, exceeds 64 bytes, starts with
/// a non-letter, or contains a character other than a letter, digit, `_`, or
/// `-` after its first byte.
pub(crate) fn validate_alias(alias: &str) -> Result<()> {
    let bytes = alias.as_bytes();
    let valid = (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphabetic()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(Error::Lua(format!(
            "invalid alias {alias:?}: expected [A-Za-z][A-Za-z0-9_-]{{0,63}}"
        )))
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
            assert!(validate_alias(alias).is_ok(), "{alias:?} must validate");
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
                validate_alias(alias).is_err(),
                "{alias:?} must not validate"
            );
        }
        let error = validate_alias("1search").expect_err("the message names the grammar");
        assert!(
            error
                .to_string()
                .contains("invalid alias \"1search\": expected [A-Za-z][A-Za-z0-9_-]{0,63}"),
            "unexpected message: {error}"
        );
    }
}
