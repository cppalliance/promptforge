//! Bundled chat-template families and conservative model resolution metadata.
//!
//! The catalog stores templates for `llama-server`; it does not render them at
//! runtime. Rendering in tests uses a Jinja2-compatible dev dependency as a
//! differential oracle.

use std::str::FromStr;

mod catalog;
#[cfg(test)]
mod live;
mod mapper_data;
mod overrides;
#[cfg(test)]
mod tests;

pub use crate::chat_templates::overrides::{KNOWN_OVERRIDES, KnownOverride, known_override};

/// A bundled chat-template family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Family {
    /// ChatML role-delimited conversations.
    Chatml,
    /// Meta Llama 3 conversations without native tool calls.
    Llama3,
    /// Meta Llama 3.1, 3.2, and 3.3 conversations.
    Llama31,
    /// Qwen 2.5 conversations.
    Qwen25,
    /// Qwen 3 conversations.
    Qwen3,
    /// Gemma 3 conversations.
    Gemma3,
    /// Gemma 4 conversations.
    Gemma4,
    /// Mistral instruction conversations.
    Mistral,
    /// Phi 3 and Phi 3.5 conversations.
    Phi3,
    /// Phi 4 conversations.
    Phi4,
    /// OpenAI GPT OSS conversations.
    GptOss,
    /// Zephyr conversations.
    Zephyr,
}

/// A failure to parse a chat-template family alias.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseFamilyError {
    /// The supplied alias does not name a bundled family.
    #[error("unknown chat-template family `{value}`")]
    #[non_exhaustive]
    Unknown {
        /// The unrecognized input.
        value: String,
    },
}

impl FromStr for Family {
    type Err = ParseFamilyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse_alias(value).ok_or_else(|| ParseFamilyError::Unknown {
            value: value.to_owned(),
        })
    }
}

/// Returns the family mapped to an exact Hugging Face repository identifier.
///
/// Matching is ASCII-case-insensitive after trimming surrounding whitespace.
/// Partial repository names and model-name heuristics are intentionally not
/// accepted.
#[must_use]
pub fn family_for_model(hint: &str) -> Option<Family> {
    let normalized = hint.trim().to_ascii_lowercase();
    mapper_data::MODEL_FAMILIES
        .binary_search_by(|(model, _)| model.cmp(&normalized.as_str()))
        .ok()
        .map(|index| mapper_data::MODEL_FAMILIES[index].1)
}

/// Returns the family mapped from a Hugging Face model download URL.
///
/// The repository identifier before `/resolve/` is passed through the same
/// exact-match mapper as [`family_for_model`].
#[must_use]
pub fn family_for_model_source(source: &str) -> Option<Family> {
    hugging_face_model_id(source).and_then(family_for_model)
}

/// Returns every exact model-to-family mapping in stable lookup order.
#[must_use]
pub fn model_family_mappings() -> &'static [(&'static str, Family)] {
    mapper_data::MODEL_FAMILIES
}

pub(crate) fn hugging_face_model_id(source: &str) -> Option<&str> {
    let path = source.strip_prefix("https://huggingface.co/")?;
    let model_end = path.find("/resolve/")?;
    let model = &path[..model_end];
    (model.split('/').count() == 2).then_some(model)
}

/// An inclusive range of validated `llama.cpp` release builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct LlamaCppBuildRange {
    /// The earliest validated release tag.
    pub first: &'static str,
    /// The latest validated release tag.
    pub last: &'static str,
}

/// The `llama.cpp` builds against which this catalog was validated.
///
/// A one-build range is deliberate. Changes to `common/chat.cpp` and the Jinja
/// engine require rerunning the ignored live parity suite before widening it.
pub const VALIDATED_LLAMA_CPP_BUILDS: LlamaCppBuildRange = LlamaCppBuildRange {
    first: "b10082",
    last: "b10082",
};
