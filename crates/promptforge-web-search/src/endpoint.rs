//! Validation of the gateway API root the provider POSTs search requests to.

/// A validated gateway API base URL (the OpenAI-shaped `/v1` root).
///
/// Construction rejects a URL without an `http`/`https` scheme or host, one
/// that embeds credentials, or one carrying a query or fragment, so the tool
/// can never be pointed at an unusable endpoint or one whose address itself
/// carries a secret. A trailing slash is trimmed so request paths join
/// cleanly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Endpoint {
    url: String,
}

impl Endpoint {
    /// Validates and normalizes a gateway base URL.
    ///
    /// Parsing goes through a strict URL type rather than a hand-rolled
    /// prefix/host scan, and the parse failure is preserved as the source.
    ///
    /// # Errors
    /// Returns an [`EndpointError`] when `url` is not a valid absolute URL,
    /// does not use an `http`/`https` scheme, names no host, embeds
    /// credentials (a `user:pass@` component), or carries a query or fragment
    /// (an API root is a bare path).
    pub(crate) fn new(url: &str) -> Result<Endpoint, EndpointError> {
        let trimmed = url.trim();
        let parsed = url::Url::parse(trimmed).map_err(EndpointError::Parse)?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(EndpointError::Scheme);
        }
        match parsed.host_str() {
            None | Some("") => return Err(EndpointError::NoHost),
            Some(_) => {}
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(EndpointError::Credentials);
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(EndpointError::QueryOrFragment);
        }
        Ok(Endpoint {
            // Normalized by the URL parser; trim the trailing slash so request
            // paths (`{base}/tools/web_search`) join cleanly.
            url: parsed.as_str().trim_end_matches('/').to_string(),
        })
    }

    /// Returns the normalized base URL.
    pub(crate) fn url(&self) -> &str {
        &self.url
    }
}

/// The reason an [`Endpoint`] could not be constructed.
///
/// The messages deliberately do not echo the rejected URL: a URL can embed
/// credentials, and diagnostics must stay secret-free.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum EndpointError {
    /// The URL did not parse.
    #[error("not a valid URL")]
    Parse(#[source] url::ParseError),
    /// The scheme was not `http` or `https`.
    #[error("must use the http or https scheme")]
    Scheme,
    /// The URL named no host.
    #[error("names no host")]
    NoHost,
    /// The URL embedded credentials.
    #[error("must not embed credentials (user:pass@)")]
    Credentials,
    /// The URL carried a query or fragment.
    #[error("must not carry a query or fragment")]
    QueryOrFragment,
}

#[cfg(test)]
mod tests {
    use super::{Endpoint, EndpointError};

    #[test]
    fn rejects_unusable_or_secret_bearing_urls() {
        assert!(matches!(
            Endpoint::new("not-a-url"),
            Err(EndpointError::Parse(_))
        ));
        assert!(matches!(Endpoint::new(""), Err(EndpointError::Parse(_))));
        assert!(matches!(
            Endpoint::new("ftp://host/v1"),
            Err(EndpointError::Scheme)
        ));
        assert!(matches!(
            Endpoint::new("http://user:pass@host/v1"),
            Err(EndpointError::Credentials)
        ));
        assert!(matches!(
            Endpoint::new("http://host/v1?q=1"),
            Err(EndpointError::QueryOrFragment)
        ));
        assert!(matches!(
            Endpoint::new("http://host/v1#frag"),
            Err(EndpointError::QueryOrFragment)
        ));
    }

    #[test]
    fn normalizes_and_preserves_the_parse_source() {
        let endpoint =
            Endpoint::new("https://gateway.example.com/v1/").expect("a valid API root is accepted");
        assert_eq!(endpoint.url(), "https://gateway.example.com/v1");

        let error = Endpoint::new("not-a-url").expect_err("an invalid URL is rejected");
        assert!(
            std::error::Error::source(&error).is_some(),
            "the url::ParseError must be preserved as the source"
        );
        assert!(
            !format!("{error:?}").contains("user:pass"),
            "diagnostics must not echo embedded credentials"
        );
    }
}
