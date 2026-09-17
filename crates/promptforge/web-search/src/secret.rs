//! The redacted bearer token the provider presents to the gateway.

use std::fmt;

/// A bearer credential whose contents never appear in `Debug`, `Display`, or
/// logs.
///
/// The token is wrapped at construction so an accidental `{:?}` or log line
/// cannot leak it; only the request builder reads the exposed value to set the
/// `Authorization` header.
#[derive(Clone)]
pub(crate) struct Token(String);

impl Token {
    /// Wraps a non-empty token so it is redacted everywhere it is formatted.
    ///
    /// # Errors
    /// Returns [`TokenError::Empty`] when `token` is empty, so the tool can
    /// never be built to authenticate with a blank bearer credential.
    pub(crate) fn new(token: impl Into<String>) -> Result<Token, TokenError> {
        let token = token.into();
        if token.is_empty() {
            return Err(TokenError::Empty);
        }
        Ok(Token(token))
    }

    /// Borrows the raw token for the `Authorization` header.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

/// The reason a [`Token`] could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum TokenError {
    /// The supplied credential was empty.
    #[error("must not be empty")]
    Empty,
}

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Token(<redacted>)")
    }
}

impl fmt::Display for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::Token;

    #[test]
    fn redacts_everywhere_and_rejects_empty() {
        let token = Token::new("super-secret-token").expect("a non-empty token is accepted");
        assert_eq!(format!("{token:?}"), "Token(<redacted>)");
        assert_eq!(format!("{token}"), "<redacted>");
        assert_eq!(token.expose(), "super-secret-token");
        assert!(Token::new("").is_err());
    }
}
