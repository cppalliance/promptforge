//! The `models` frontmatter key: declared model roles.
//!
//! A role is a slot: a fill function at prepare maps it to a concrete model
//! and checks the hard keywords and the context minimum against the filled
//! descriptor. Parse only validates shape and exposes the declaration.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use serde::Deserialize;

use super::deserialize_contract_map;

/// The closed model-keyword vocabulary.
///
/// Hard keywords (`thinking`, `no-thinking`) and the context minimum are
/// checked per slot against the filled model's descriptor at prepare; soft
/// keywords document author intent. Unknown keywords are parse errors;
/// adding a keyword is a language change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ModelKeyword {
    /// The model supports extended thinking (hard).
    Thinking,
    /// The model never thinks (hard).
    NoThinking,
    /// A frontier-capability model (soft).
    Frontier,
    /// A fast model (soft).
    Fast,
    /// A small model (soft).
    Small,
    /// A creative model (soft).
    Creative,
    /// A chat-tuned model (soft).
    Chat,
}

/// One declared model role: keywords, a context minimum, and a description.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ModelRole {
    /// The declared keywords (closed vocabulary).
    #[serde(default)]
    keywords: Vec<ModelKeyword>,
    /// The minimum context window in tokens.
    #[serde(default)]
    min_context: Option<NonZeroU32>,
    /// The role's prose description.
    #[serde(default)]
    description: Option<String>,
}

impl ModelRole {
    /// Returns the declared keywords.
    #[must_use]
    pub fn keywords(&self) -> &[ModelKeyword] {
        &self.keywords
    }

    /// Returns the context minimum in tokens, when declared.
    #[must_use]
    pub fn min_context(&self) -> Option<NonZeroU32> {
        self.min_context
    }

    /// Returns the role's description, when declared.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

/// The prompt's declared model roles: label to role.
///
/// Labels are prompt-local (the alias grammar); the model never sees a
/// concrete model id in the declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModelRoles {
    roles: BTreeMap<String, ModelRole>,
}

impl ModelRoles {
    /// Returns the role declared under `label`, when present.
    #[must_use]
    pub fn get(&self, label: &str) -> Option<&ModelRole> {
        self.roles.get(label)
    }

    /// Iterates the declared roles as `(label, role)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ModelRole)> {
        self.roles
            .iter()
            .map(|(label, role)| (label.as_str(), role))
    }

    /// Returns the number of declared roles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.roles.len()
    }

    /// Returns whether no roles are declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }
}

impl<'de> Deserialize<'de> for ModelRoles {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let roles = deserialize_contract_map(deserializer, "model role label", None)?;
        Ok(ModelRoles { roles })
    }
}
