//! The `web_fetch` error type.
//!
//! [`FetchError`] is the single error type for every `web_fetch` failure mode:
//! the URL and address policies, redirects, DNS, the size caps, the timeouts,
//! and the content-type and decoding refusals. It carries a model-facing
//! rendering that withholds internal detail, and converts into
//! [`promptforge_core::Error`], which is what the `Tool` trait returns.

use std::net::IpAddr;

use promptforge_core::Error;

/// An error produced while fetching a URL.
///
/// The [`Display`](std::fmt::Display) rendering is the full log text; the
/// [`FetchError::model_facing`] rendering is what a model should see. They
/// diverge for [`FetchError::BlockedAddress`], whose log text names the resolved
/// address and the range that blocked it while its model-facing text says only
/// that the host is not fetchable.
///
/// The type is `Send + Sync + 'static` and never exposes a dependency's error
/// type: a URL parse failure is captured as a string, not as [`url::ParseError`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FetchError {
    /// The URL could not be parsed.
    #[non_exhaustive]
    #[error("invalid url: {0}")]
    InvalidUrl(String),

    /// The URL's scheme is not permitted by the policy.
    #[non_exhaustive]
    #[error("scheme not allowed: {0}")]
    BlockedScheme(String),

    /// The URL carries userinfo (a `user:pass@` component).
    #[error("url must not contain userinfo")]
    Userinfo,

    /// The URL's effective port is not on the allowlist.
    #[non_exhaustive]
    #[error("port not allowed: {0}")]
    BlockedPort(u16),

    /// The URL's host is a bare IP literal, which the policy forbids.
    #[non_exhaustive]
    #[error("ip literal host not allowed: {0}")]
    IpLiteral(String),

    /// A host resolved to an address inside a blocked range.
    ///
    /// The full [`Display`](std::fmt::Display) names the host, the resolved
    /// address, and the range, for the log. [`FetchError::model_facing`] omits
    /// the address and range so no internal topology reaches the model.
    #[non_exhaustive]
    #[error("host {host} resolved to blocked address {addr} in range {range}")]
    BlockedAddress {
        /// The host that was being resolved.
        host: String,
        /// The resolved address that fell in a blocked range.
        addr: IpAddr,
        /// The CIDR range that blocked the address.
        range: String,
    },

    /// A host resolved to no address the policy would allow.
    #[non_exhaustive]
    #[error("host {host} has no allowed address")]
    NoAllowedAddress {
        /// The host that resolved only to blocked (or zero) addresses.
        host: String,
    },

    /// A redirect hop was refused by the redirect policy.
    #[non_exhaustive]
    #[error("redirect from {from} to {to} refused: {reason}")]
    RedirectRefused {
        /// The URL the redirect came from.
        from: String,
        /// The URL the redirect pointed to.
        to: String,
        /// Why the hop was refused (downgrade, bad scheme, port, cap, ...).
        reason: String,
    },

    /// The response body exceeded the byte cap.
    ///
    /// The cap is counted on decompressed bytes, so a compressed payload that
    /// expands past `limit` is refused on its expanded size. The model-facing
    /// text names both the cap and the URL so the model can retry a smaller or
    /// different resource.
    #[non_exhaustive]
    #[error("response from {url} exceeds the {limit}-byte size cap")]
    TooLarge {
        /// The URL whose response exceeded the cap.
        url: String,
        /// The byte cap that was exceeded.
        limit: usize,
    },

    /// DNS resolution for a host failed at the system level.
    #[non_exhaustive]
    #[error("dns resolution failed for {host}: {message}")]
    Dns {
        /// The host whose resolution failed.
        host: String,
        /// The underlying failure text.
        message: String,
    },

    /// The response declared a content type this tool does not return as text.
    ///
    /// PDFs and every binary type (octet-stream, images, audio, video,
    /// archives) land here. The message names the type and points the model at
    /// a next move so it does not simply retry the same URL.
    #[non_exhaustive]
    #[error(
        "content type {content_type} from {url} cannot be returned as text; try an HTML version of the page or a different URL"
    )]
    UnsupportedContentType {
        /// The URL whose response carried the unsupported type.
        url: String,
        /// The unsupported content type, verbatim from the response header.
        content_type: String,
    },

    /// The response carried no `Content-Type` header.
    ///
    /// The tool refuses to sniff an absent type, so the URL is rejected rather
    /// than guessed at. The message points the model at a URL that declares a
    /// type.
    #[non_exhaustive]
    #[error(
        "response from {url} declared no content type; refusing to guess its format; try a different URL"
    )]
    NoContentType {
        /// The URL whose response carried no content type.
        url: String,
    },

    /// The request exceeded its time budget.
    ///
    /// Either the connection could not be established in time or the total
    /// request took longer than the configured timeout. The message names the
    /// URL and suggests a retry, since a timeout is often transient.
    #[non_exhaustive]
    #[error("request to {url} timed out; try again or use a different URL")]
    Timeout {
        /// The URL whose request timed out.
        url: String,
    },

    /// The response declared a charset that could not be decoded.
    ///
    /// The charset label was not one the decoder recognizes, so the bytes
    /// cannot be turned into text. The message names the label so the failure
    /// is actionable.
    #[non_exhaustive]
    #[error("response from {url} declared unknown charset {charset}; cannot decode its text")]
    Undecodable {
        /// The URL whose response declared the charset.
        url: String,
        /// The unrecognized charset label, verbatim from the response header.
        charset: String,
    },
}

impl FetchError {
    /// Renders the error as text safe to return to the model.
    ///
    /// Identical to the [`Display`](std::fmt::Display) output for every variant
    /// except [`FetchError::BlockedAddress`], whose model-facing text omits the
    /// resolved address and the blocking range: the model learns only that the
    /// host is not fetchable, never the internal topology behind it.
    #[must_use]
    pub fn model_facing(&self) -> String {
        match self {
            FetchError::BlockedAddress { host, .. } => {
                format!("host {host} is not fetchable")
            }
            other => other.to_string(),
        }
    }
}

impl From<FetchError> for Error {
    /// Maps a fetch failure onto the core error the `Tool` trait returns.
    ///
    /// URL-policy rejections are input errors caught before any network access,
    /// so they map to [`Error::Parse`] carrying the model-facing text.
    fn from(err: FetchError) -> Error {
        Error::Parse(err.model_facing())
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::FetchError;

    #[test]
    fn blocked_address_log_keeps_detail_model_facing_hides_it() {
        let addr: IpAddr = "169.254.169.254".parse().expect("test address parses");
        let err = FetchError::BlockedAddress {
            host: "metadata.internal".to_string(),
            addr,
            range: "169.254.0.0/16".to_string(),
        };

        // The full Display (the log rendering) names the address and the range.
        let log = err.to_string();
        assert!(log.contains("169.254.169.254"), "log must name the address");
        assert!(log.contains("169.254.0.0/16"), "log must name the range");

        // The model-facing rendering omits both and says only the host is not
        // fetchable.
        let facing = err.model_facing();
        assert!(
            !facing.contains("169.254.169.254") && !facing.contains("169.254.0.0/16"),
            "model-facing text must hide the address and range, got: {facing}"
        );
        assert!(
            facing.contains("metadata.internal") && facing.contains("not fetchable"),
            "model-facing text should name the host as not fetchable, got: {facing}"
        );
    }
}
