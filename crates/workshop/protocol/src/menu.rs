//! Inbound Model-menu frames: the profile selection event.

use serde::{Deserialize, Deserializer};

/// The inbound profile selection: `{"type":"switch_profile","name":...}`.
///
/// `name` is a profile name, or `null` to select no profile; the key
/// itself is required, so a frame that omits it is malformed rather than
/// a no-profile selection. The session routes on the envelope's `type`
/// and deserializes the body with serde, which ignores the envelope tag
/// and the optional `id` the session echoes on a refusal. Like every
/// inbound frame it takes no delivery classification, because the server
/// pushes none.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct SwitchProfileFrame {
    /// The profile to select, or `None` (`null` on the wire) for no
    /// profile.
    #[serde(deserialize_with = "required_nullable")]
    pub name: Option<String>,
}

/// Deserializes a nullable string whose key must be present: serde's
/// derive treats a missing `Option` field as `None` unless the field
/// names its own deserializer, and this field names one, so an absent
/// `name` fails instead of selecting no profile.
fn required_nullable<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
}
