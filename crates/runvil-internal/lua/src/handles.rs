//! Tool bindings, the shared tool set, and the per-binding output kind that
//! shape how bound tools reach Lua, and the section text a prompt read sees.

use std::collections::BTreeMap;

use mlua::Value;
use runvil_types::plugins::PluginId;
use runvil_types::tools::{ToolDescriptor, ToolId};
use serde_json::Value as Json;

/// How a bound tool's output resumes into Lua at the `tools.call` boundary.
///
/// Declared on the binding (from the descriptor's `structured_output`), not
/// the tool implementation, so the caller's tool catalog decides how scripts
/// receive the output. The model tool loop never consults the kind (its
/// results are always added to the conversation as text).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolOutputKind {
    /// The output text resumes as a Lua string.
    #[default]
    Plain,
    /// The output text is JSON, parsed at dispatch and resumed as a Lua
    /// table through the serde boundary; invalid JSON is the tool's error.
    Structured,
}

/// One catalog tool bound under the name a model sees, holding the tool's
/// data - its schema, description, and output kind - and never its
/// implementation.
///
/// Run-time execution (schema preparation, script dispatch) reads the
/// binding alone; a call is issued as an effect naming the identity, and
/// performing it is the Harness's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolBinding {
    /// The model-facing name: an offered tool's wire name, or the full id
    /// of a catalog tool a script reaches without an offer.
    alias: String,
    /// The tool's own catalog description.
    description: String,
    /// The selected stable live identity.
    id: ToolId,
    /// The JSON-Schema `object` the tool's arguments must match, advertised
    /// under the alias.
    schema: Json,
    /// How a script-initiated `tools.call` resumes this binding's output;
    /// the model tool loop ignores it.
    output_kind: ToolOutputKind,
}

impl ToolBinding {
    /// Binds `alias` to the tool `descriptor` describes: the description is
    /// the tool's own, and the output kind follows the descriptor's
    /// structured-output flag.
    #[must_use]
    pub fn from_descriptor(alias: &str, descriptor: &ToolDescriptor) -> Self {
        Self {
            alias: alias.to_owned(),
            description: descriptor.description.clone(),
            id: descriptor.id.clone(),
            schema: descriptor.parameters_schema.clone(),
            output_kind: if descriptor.structured_output {
                ToolOutputKind::Structured
            } else {
                ToolOutputKind::Plain
            },
        }
    }

    /// Returns the model-facing name.
    #[must_use]
    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// Returns the tool's catalog description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the selected stable live identity.
    #[must_use]
    pub fn id(&self) -> &ToolId {
        &self.id
    }

    /// Returns the JSON-Schema `object` the tool's arguments must match.
    #[must_use]
    pub fn schema(&self) -> &Json {
        &self.schema
    }

    /// Returns how a script-initiated `tools.call` resumes this binding's
    /// output.
    #[must_use]
    pub fn output_kind(&self) -> ToolOutputKind {
        self.output_kind
    }
}

/// Outcome of a Lua block that may invoke `jump`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LuaBlockResult {
    /// Normal completion with an optional scalar return.
    Returned(Option<String>),
    /// `jump` transferred control to this heading (`## Name`).
    Jump(String),
}

/// Resolves a `call` / `jump` target from a heading string.
///
/// # Errors
/// Returns a Lua error when the value is not a string.
pub(crate) fn resolve_section_target(value: Value) -> mlua::Result<String> {
    match value {
        Value::String(s) => Ok(s.to_str()?.to_owned()),
        other => Err(mlua::Error::external(format!(
            "section target must be a string, got {}",
            other.type_name()
        ))),
    }
}

/// What a prompt read sees of one visible section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionText {
    /// The section's own raw Markdown.
    pub markdown: String,
    /// Its pre-parsed list items; empty for a section that isn't a list.
    pub items: Vec<String>,
}

/// Which library a `require` resolved to, opaque to this crate.
///
/// The caller encodes a library section's position in the identity
/// string; `require`'s cache keys on that identity, never on the heading
/// text, so one heading resolved from two positions names two libraries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LibraryKey(String);

impl LibraryKey {
    /// Builds a key whose stable identity is `identity`.
    #[must_use]
    pub fn new(identity: String) -> Self {
        Self(identity)
    }

    /// Returns the identity the key was built from.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.0
    }
}

/// What `require` loads from one library section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    /// Which library this is: the cache key, and the position a `require`
    /// made while it loads resolves from.
    pub key: LibraryKey,
    /// The library section's heading with its markers, such as
    /// `## helpers`, which its load reports and a cycle error name.
    pub heading: String,
    /// The section's blocks in order.
    pub blocks: Vec<LibraryBlock>,
}

/// One block of a library section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryBlock {
    /// The block's compiled Lua.
    pub program: crate::LuaProgram,
    /// The block's prose template, the Markdown before it.
    pub prose: String,
}

/// The run's tool set: the declared Plugins, the run's roster, and every
/// catalog tool the run can offer. Nothing changes it during a run, so the
/// run and every section VM share one `Arc<ToolSet>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolSet {
    /// The Plugins the frontmatter declares, in declaration order.
    declared: Vec<PluginId>,
    /// Every installed Plugin the caller reports, declared or not, with or
    /// without tools: no reason when it can serve the run, and the reason,
    /// written for the model, when it can't.
    roster: BTreeMap<PluginId, Option<String>>,
    /// Every catalog tool the run can offer, in id order, each bound under
    /// its wire name.
    offered: Vec<ToolBinding>,
}

impl ToolSet {
    /// Builds the run's tool set from the declared Plugins in declaration
    /// order, the roster, and the offering in tool id order.
    #[must_use]
    pub fn new(
        declared: Vec<PluginId>,
        roster: BTreeMap<PluginId, Option<String>>,
        offered: Vec<ToolBinding>,
    ) -> Self {
        Self {
            declared,
            roster,
            offered,
        }
    }

    /// Returns the declared plugins in declaration order.
    #[must_use]
    pub fn declared(&self) -> &[PluginId] {
        &self.declared
    }

    /// Returns every installed Plugin the caller reports, with the reason,
    /// written for the model, when it cannot serve the run.
    #[must_use]
    pub fn roster(&self) -> &BTreeMap<PluginId, Option<String>> {
        &self.roster
    }

    /// Returns the offering in tool id order.
    #[must_use]
    pub fn offered(&self) -> &[ToolBinding] {
        &self.offered
    }

    /// Returns the offered binding whose wire name or canonical id is
    /// `name`, which matches at most one way because wire names and local
    /// aliases never contain `/` and ids always do.
    #[must_use]
    pub fn offered_binding(&self, name: &str) -> Option<&ToolBinding> {
        if name.contains('/') {
            let id = ToolId::parse(name).ok()?;
            self.offered.iter().find(|binding| *binding.id() == id)
        } else {
            self.wire_binding(name)
        }
    }

    /// Returns the offered binding whose wire name is `name`, never matching
    /// a canonical id, because a model reaches tools only by the wire names
    /// a round advertised.
    #[must_use]
    pub fn wire_binding(&self, name: &str) -> Option<&ToolBinding> {
        self.offered.iter().find(|binding| binding.alias() == name)
    }
}

#[cfg(test)]
#[path = "handles-tests.rs"]
mod tests;
