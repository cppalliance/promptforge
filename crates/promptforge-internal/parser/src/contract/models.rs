//! Parses the `models` frontmatter key into declared model roles.
//!
//! A role is a slot: a fill function at prepare maps it to a concrete model
//! and checks the hard keywords and the context minimum against the filled
//! descriptor. Parse only validates shape and exposes the declaration.

use std::collections::{BTreeMap, btree_map};
use std::num::NonZeroU32;

use serde::Deserialize;

use crate::contract::{self, ContractKeys};

/// A keyword that describes the model a role needs.
///
/// Keywords are hard or soft. When a run is prepared, each role's hard
/// keywords (`thinking` and `no-thinking`) and its minimum context window
/// are checked against the descriptor of the model that fills the role.
/// Soft keywords document the author's intent.
///
/// The set of keywords is fixed, and a keyword outside the set is a parse
/// error. Adding a keyword is a change to the prompt language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ModelKeyword {
    /// A hard keyword: the model supports extended thinking.
    Thinking,
    /// A hard keyword: the model runs with thinking turned off.
    NoThinking,
    /// A soft keyword: a frontier-capability model.
    Frontier,
    /// A soft keyword: a fast model.
    Fast,
    /// A soft keyword: a small model.
    Small,
    /// A soft keyword: a creative model.
    Creative,
    /// A soft keyword: a chat-tuned model.
    Chat,
}

/// One model role a prompt declares: its keywords, minimum context window,
/// and description.
///
/// A role is a slot for a model the prompt needs. When the run is
/// prepared, a concrete model fills the slot.
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

    /// Returns the minimum context window in tokens, when declared.
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

/// The model roles a prompt declares under the `models` frontmatter key,
/// keyed by label.
///
/// A label is a name local to the prompt. It follows the same grammar as
/// a local tool alias: `[A-Za-z][A-Za-z0-9_-]{0,63}`.
///
/// A script reaches a role's model handle with `models.get(label)`. A label
/// is never a Lua global, so it may be a reserved name.
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

    /// Iterates the declared roles as `(label, role)` pairs, in label order.
    pub fn iter(&self) -> ModelRolesIter<'_> {
        ModelRolesIter {
            roles: self.roles.iter(),
        }
    }

    /// Returns the number of declared roles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.roles.len()
    }

    /// Returns whether the set of declared roles is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }
}

impl<'a> IntoIterator for &'a ModelRoles {
    type Item = (&'a str, &'a ModelRole);
    type IntoIter = ModelRolesIter<'a>;

    fn into_iter(self) -> ModelRolesIter<'a> {
        self.iter()
    }
}

/// Yields the declared roles of one [`ModelRoles`] as `(label, role)` pairs,
/// in label order.
///
/// [`ModelRoles::iter`] returns it.
#[derive(Debug, Clone)]
#[must_use = "an iterator does nothing until it is consumed"]
pub struct ModelRolesIter<'a> {
    roles: btree_map::Iter<'a, String, ModelRole>,
}

impl<'a> Iterator for ModelRolesIter<'a> {
    type Item = (&'a str, &'a ModelRole);

    fn next(&mut self) -> Option<(&'a str, &'a ModelRole)> {
        self.roles
            .next()
            .map(|(label, role)| (label.as_str(), role))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.roles.size_hint()
    }
}

impl ExactSizeIterator for ModelRolesIter<'_> {}

impl<'de> Deserialize<'de> for ModelRoles {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let roles = contract::deserialize_contract_map(
            deserializer,
            ContractKeys {
                what: "model role label",
            },
        )?;
        Ok(ModelRoles { roles })
    }
}
