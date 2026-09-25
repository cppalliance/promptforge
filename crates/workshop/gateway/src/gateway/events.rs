//! The gateway client's wire types: the buffered relay response, the
//! forwarded config-panel response, and the profile-selection outcome.

use serde::Deserialize;

/// A gateway HTTP response captured for verbatim relay.
#[derive(Debug)]
pub struct GatewayResponse {
    /// The gateway's status code, relayed unchanged.
    pub status: reqwest::StatusCode,
    /// The gateway's response body, relayed byte-for-byte.
    pub body: Vec<u8>,
}

/// A gateway response captured for the config-panel proxy: the relay
/// keeps the content type alongside the status and body, because the
/// config UI distinguishes a buffered JSON answer from an SSE stream by
/// it.
#[derive(Debug)]
pub struct ForwardedResponse {
    /// The gateway's status code, relayed unchanged.
    pub status: reqwest::StatusCode,
    /// The gateway's `Content-Type`, when it sent one.
    pub content_type: Option<String>,
    /// The gateway's response body, relayed byte-for-byte.
    pub body: Vec<u8>,
}

/// The gateway's answer to a profile selection, `POST /admin/switch-profile`.
///
/// An accepted selection answers one JSON document decoding as
/// [`SwitchProfileBody`]: the gateway persisted the selection and reports
/// whether it must restart to load it. A refusal (bad auth, a malformed or
/// undefined name) is buffered rather than reported as an error, matching
/// the relay contract of the other client methods.
#[derive(Debug)]
#[non_exhaustive]
pub enum SwitchResponse {
    /// The gateway accepted and persisted the selection.
    Selected(SwitchProfileBody),

    /// A refusal, buffered: the gateway's error envelope.
    Buffered(GatewayResponse),
}

/// The body of an accepted profile selection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SwitchProfileBody {
    /// The selection now persisted: a profile name, or `None` for no
    /// profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Whether the gateway must restart before the selection is served;
    /// `false` when the selection already matches the running profile.
    pub restart_required: bool,
}
