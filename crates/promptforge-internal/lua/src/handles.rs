//! Tool bindings, the shared tool set, and the per-binding output kind that shape how bound tools reach Lua.

use promptforge_types::capabilities::CapabilityId;
use promptforge_types::tools::ToolDescriptor;

use super::{Error, Json, Mutex, Result, ToolId, Value};

/// How a bound tool's output resumes into Lua at the `tools.call` boundary.
///
/// Declared on the binding, not the tool implementation, so a host decides
/// per binding how scripts receive the output. Every existing tool is
/// [`Plain`](ToolOutputKind::Plain); the model tool loop never consults the
/// kind (its results are always added to the conversation as text).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolOutputKind {
    /// The output text resumes as a Lua string - every existing tool,
    /// unchanged behavior.
    #[default]
    Plain,
    /// The output text is JSON, parsed at dispatch and resumed as a Lua
    /// table through the serde boundary; invalid JSON is the tool's error.
    Structured,
}

/// One prompt-local alias bound to one stable live tool identity, holding
/// the tool's data - its schema, description, output kind, and the
/// contributing capability's conflicts - and never its implementation.
///
/// Run-time execution (schema preparation, script dispatch) reads the
/// binding alone; a call is issued as an effect naming the identity, and
/// the host resolves the implementation against its own table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolBinding {
    /// The exact prompt-local alias.
    pub alias: String,
    /// The slot's description: the tool's own.
    pub description: String,
    /// The selected stable live identity.
    pub id: ToolId,
    /// Author override for the model-facing schema description.
    ///
    /// When set, the executor advertises this instead of the bound tool's
    /// default description.
    pub model_description: Option<String>,
    /// The JSON-Schema `object` the tool's arguments must match, advertised
    /// under the alias.
    pub schema: Json,
    /// How a script-initiated `tools.call` resumes this binding's output;
    /// the model tool loop ignores it.
    pub output_kind: ToolOutputKind,
    /// The co-activation conflicts of the capability that contributed the
    /// tool, kept for the record.
    pub conflicts: Vec<CapabilityId>,
}

impl ToolBinding {
    /// Binds `alias` to the tool `descriptor` describes: the slot's
    /// description is the tool's own, the output kind follows the
    /// descriptor's structured-output flag, and no override is set.
    #[must_use]
    pub fn from_descriptor(alias: &str, descriptor: &ToolDescriptor) -> Self {
        Self {
            alias: alias.to_owned(),
            description: descriptor.description.clone(),
            id: descriptor.id.clone(),
            model_description: None,
            schema: descriptor.parameters_schema.clone(),
            output_kind: if descriptor.structured_output {
                ToolOutputKind::Structured
            } else {
                ToolOutputKind::Plain
            },
            conflicts: descriptor.conflicts.clone(),
        }
    }

    /// Builds a binding for a test double: the identity and schema come
    /// from the descriptor, the slot's description is `description`, with
    /// no override.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for `promptforge-engine`'s executor
    /// tests, not host API.
    #[doc(hidden)]
    #[must_use]
    pub fn for_test(alias: &str, description: &str, descriptor: &ToolDescriptor) -> Self {
        Self {
            description: description.to_owned(),
            ..Self::from_descriptor(alias, descriptor)
        }
    }

    /// Returns the exact prompt-local alias.
    #[must_use]
    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// Returns the declared capability description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the selected stable live identity.
    #[must_use]
    pub fn id(&self) -> &ToolId {
        &self.id
    }

    /// Returns the author override for the model-facing description, if any.
    #[must_use]
    pub fn model_description(&self) -> Option<&str> {
        self.model_description.as_deref()
    }

    /// Returns the JSON-Schema `object` the tool's arguments must match.
    #[must_use]
    pub fn schema(&self) -> &Json {
        &self.schema
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

/// The run's tool set: the frontmatter's filled tool slots plus the
/// prompt-wide `always` aliases.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolSet {
    /// The prompt-level bindings in declaration order.
    pub bindings: Vec<ToolBinding>,
    /// The prompt-wide `always` aliases in declaration order.
    pub always: Vec<String>,
}

impl ToolSet {
    /// Builds a set from owned parts, for executor test doubles.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for `promptforge-engine`'s executor
    /// tests, not host API.
    #[doc(hidden)]
    #[must_use]
    pub fn for_test(bindings: Vec<ToolBinding>, always: Vec<String>) -> Self {
        Self { bindings, always }
    }

    /// Reassembles a set from owned snapshots of its two lists (the
    /// [`ToolView`] read pair).
    #[must_use]
    pub fn from_parts(bindings: Vec<ToolBinding>, always: Vec<String>) -> Self {
        Self { bindings, always }
    }

    /// Returns bindings in declaration order.
    #[must_use]
    pub fn bindings(&self) -> &[ToolBinding] {
        &self.bindings
    }

    /// Returns prompt-wide aliases in declaration order.
    #[must_use]
    pub fn always(&self) -> &[String] {
        &self.always
    }

    /// Returns the binding for `alias`, if it was declared.
    #[must_use]
    pub fn binding(&self, alias: &str) -> Option<&ToolBinding> {
        self.bindings.iter().find(|binding| binding.alias == alias)
    }
}

/// The read-only view over the run's [`ToolSet`].
///
/// The run context shares the set as `Arc<dyn ToolView>`; section VMs share
/// the same allocation through concrete `Arc<Mutex<ToolSet>>` handles, with
/// `tools.always` the only writer (a prompt-wide fact). Every method locks
/// briefly and returns an owned snapshot: a mutex guard cannot outlive the
/// call.
pub trait ToolView: Send + Sync {
    /// Returns an owned snapshot of the bindings in declaration order.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the set's mutex is poisoned.
    fn bindings(&self) -> Result<Vec<ToolBinding>>;

    /// Returns an owned snapshot of the prompt-wide `always` aliases in
    /// declaration order.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the set's mutex is poisoned.
    fn always(&self) -> Result<Vec<String>>;

    /// Returns an owned clone of the binding for `alias`, if it was
    /// declared.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the set's mutex is poisoned.
    fn binding(&self, alias: &str) -> Result<Option<ToolBinding>>;
}

/// Maps a poisoned set lock to [`Error::Lua`], matching every other mutex
/// in the Lua host layer.
fn lock_tool_set(set: &Mutex<ToolSet>) -> Result<std::sync::MutexGuard<'_, ToolSet>> {
    set.lock()
        .map_err(|_| Error::Lua("tool set mutex was poisoned".to_owned()))
}

impl ToolView for Mutex<ToolSet> {
    fn bindings(&self) -> Result<Vec<ToolBinding>> {
        Ok(lock_tool_set(self)?.bindings.clone())
    }

    fn always(&self) -> Result<Vec<String>> {
        Ok(lock_tool_set(self)?.always.clone())
    }

    fn binding(&self, alias: &str) -> Result<Option<ToolBinding>> {
        Ok(lock_tool_set(self)?.binding(alias).cloned())
    }
}

#[cfg(test)]
#[path = "handles-tests.rs"]
mod tests;
