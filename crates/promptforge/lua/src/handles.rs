use super::{Arc, Error, Mutex, Result, Tool, ToolId, Value};

/// How a bound tool's output resumes into Lua at the `tools.call` boundary.
///
/// Declared on the binding, not the tool implementation, so a host decides
/// per binding how scripts receive the output. Every existing tool is
/// [`Plain`](ToolOutputKind::Plain); the model tool loop never consults the
/// kind (its results always ride the conversation as text).
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

/// One prompt-local alias bound to one stable live tool identity, carrying
/// the resolved implementation attached when the slot was filled.
///
/// The implementation rides with the binding so run-time execution (schema
/// preparation, dispatch) never consults the assembled catalog again.
#[derive(Clone)]
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
    /// The resolved implementation, attached at fill time.
    pub tool: Arc<dyn Tool>,
    /// How a script-initiated `tools.call` resumes this binding's output;
    /// the model tool loop ignores it.
    pub output_kind: ToolOutputKind,
}

/// Equality is keyed on the binding's data (alias, capability text, stable
/// identity, override); the attached implementation is a trait object and
/// takes no part in comparison.
impl PartialEq for ToolBinding {
    fn eq(&self, other: &Self) -> bool {
        self.alias == other.alias
            && self.description == other.description
            && self.id == other.id
            && self.model_description == other.model_description
            && self.output_kind == other.output_kind
    }
}

impl Eq for ToolBinding {}

/// Shows the stable identity, never the trait object.
impl std::fmt::Debug for ToolBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolBinding")
            .field("alias", &self.alias)
            .field("description", &self.description)
            .field("id", &self.id)
            .field("model_description", &self.model_description)
            .field("output_kind", &self.output_kind)
            .finish_non_exhaustive()
    }
}

impl ToolBinding {
    /// Builds a binding for a test double: the identity comes from the tool,
    /// with no override.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for `promptforge-api-runtime`'s executor
    /// tests, not host API.
    #[doc(hidden)]
    #[must_use]
    pub fn for_test(alias: &str, description: &str, tool: Arc<dyn Tool>) -> Self {
        Self {
            alias: alias.to_owned(),
            description: description.to_owned(),
            id: tool.id(),
            model_description: None,
            tool,
            output_kind: ToolOutputKind::default(),
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

    /// Returns the resolved implementation attached at bind time.
    #[must_use]
    pub fn tool(&self) -> &dyn Tool {
        self.tool.as_ref()
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
    /// `#[doc(hidden)]`: a cross-crate seam for `promptforge-api-runtime`'s executor
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
/// `tools.always` the only writer (a prompt-wide fact). The trait exposes no
/// mutation. Every method locks briefly and returns an owned snapshot: a
/// mutex guard cannot outlive the call.
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
