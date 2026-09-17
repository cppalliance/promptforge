//! Hash-first overrides for known broken embedded GGUF templates.

use sha2::{Digest as _, Sha256};

use super::Family;

const EDGE_OVERRIDE: usize = 0;
const STANDARD_OVERRIDE: usize = 1;

/// A bundled replacement for a known broken embedded GGUF template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct KnownOverride {
    /// SHA-256 of the exact broken embedded template bytes.
    pub embedded_template_sha256: &'static str,
    /// The catalog family associated with the replacement.
    pub family: Family,
    /// Stable asset filename for staging in a later resolution step.
    pub asset_name: &'static str,
    /// Exact bundled replacement template.
    pub template: &'static str,
}

/// Known broken embedded templates and their bundled replacements.
///
/// The two hashes cover all twelve revision-pinned Gemma 4 repositories in
/// the catalog research. Lookup always checks these hashes before considering
/// a model identifier.
pub static KNOWN_OVERRIDES: &[KnownOverride] = &[
    KnownOverride {
        embedded_template_sha256: "241c50d86bdfe5e43307da87f559cd2416aacd67a8de46c15acc0105ef2200b7",
        family: Family::Gemma4,
        asset_name: "gemma-4-edge.jinja",
        template: include_str!("assets/overrides/gemma-4-edge.jinja"),
    },
    KnownOverride {
        embedded_template_sha256: "845f1ee48e39fc942fe190da9df6a1c5db229e17a96ea08966ad1c9274e73d1b",
        family: Family::Gemma4,
        asset_name: "gemma-4-standard.jinja",
        template: include_str!("assets/overrides/gemma-4-standard.jinja"),
    },
];

/// Resolves a bundled override from embedded template bytes, then model ID.
///
/// Exact content hashes take precedence because repository names can be
/// incomplete or misleading. The model-ID path retains the open-ended
/// `unsloth/gemma-4-*-GGUF` policy for future repositories whose embedded
/// template hash could not have been cataloged in advance.
#[must_use]
pub fn known_override(
    embedded_template: Option<&str>,
    model_hint: Option<&str>,
) -> Option<&'static KnownOverride> {
    if let Some(template) = embedded_template {
        let digest = sha256_hex(template);
        if let Some(known) = KNOWN_OVERRIDES
            .iter()
            .find(|known| known.embedded_template_sha256 == digest)
        {
            return Some(known);
        }
    }
    model_hint.and_then(override_for_model)
}

pub(super) fn sha256_hex(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(value.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn override_for_model(model_hint: &str) -> Option<&'static KnownOverride> {
    let normalized = model_hint.trim().to_ascii_lowercase();
    let canonical = if normalized.contains('/') {
        normalized
    } else {
        format!("unsloth/{normalized}")
    };
    let variant = canonical
        .strip_prefix("unsloth/gemma-4-")?
        .strip_suffix("-gguf")?;
    if variant.is_empty() {
        return None;
    }
    if matches!(variant, "e2b-it" | "e4b-it")
        || variant.starts_with("e2b-it-")
        || variant.starts_with("e4b-it-")
    {
        Some(&KNOWN_OVERRIDES[EDGE_OVERRIDE])
    } else {
        Some(&KNOWN_OVERRIDES[STANDARD_OVERRIDE])
    }
}
