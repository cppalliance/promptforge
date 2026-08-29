use super::{
    Arc, Error, Json, LuaSerdeExt, MetaMethod, Mutex, Result, Tool, ToolId, UserData,
    UserDataFields, UserDataMethods, Value, json,
};

/// Resolves one plain-English capability description to one stable live tool.
///
/// This is the deterministic seam used by live H1 resolution. It keeps core
/// independent of any concrete picker implementation while allowing a caller
/// to supply a fixed resolver in tests.
pub trait ToolResolver: Send + Sync {
    /// Resolves `description` to a stable tool identity.
    ///
    /// # Errors
    /// Returns a core error when the capability cannot be resolved uniquely.
    fn resolve(&self, description: &str) -> Result<ToolId>;

    /// Reports the near-duplicate pairs among the bound `ids` as
    /// `(first, second, similarity)` triples, for the bind-time conflict
    /// scan.
    ///
    /// The default reports no pairs: a resolver without similarity
    /// knowledge (a fixed test resolver) records no conflicts.
    ///
    /// # Errors
    /// Returns a core error when the analysis backend fails.
    fn near_duplicates(&self, ids: &[ToolId]) -> Result<Vec<(ToolId, ToolId, f32)>> {
        let _ = ids;
        Ok(Vec::new())
    }
}

impl<F> ToolResolver for F
where
    F: Fn(&str) -> Result<ToolId> + Send + Sync,
{
    fn resolve(&self, description: &str) -> Result<ToolId> {
        self(description)
    }
}

/// One near-duplicate clash recorded at bind time.
///
/// The picker is an H1-phase capability, so the score is copied onto the
/// binding when the clash is recorded; it cannot be recomputed later.
#[derive(Debug, Clone)]
pub struct Conflict {
    /// The alias of the other binding in the clashing pair.
    pub alias: String,
    /// The picker's cosine similarity between the two bound tools.
    pub similarity: f64,
}

/// Bit comparison on the score keeps equality reflexive (`f64 ==` is not,
/// at NaN), which [`ToolBinding`]'s `Eq` relies on.
impl PartialEq for Conflict {
    fn eq(&self, other: &Self) -> bool {
        self.alias == other.alias && self.similarity.to_bits() == other.similarity.to_bits()
    }
}

impl Eq for Conflict {}

/// One prompt-local alias bound to one stable live tool identity, carrying
/// the resolved implementation attached at bind time.
///
/// The implementation rides with the binding so post-H1 execution (schema
/// preparation, dispatch) never consults the implementation catalog again: a
/// capability whose tool is unavailable fails at the `tools.bind` call, before
/// any binding exists.
#[derive(Clone)]
pub struct ToolBinding {
    /// The exact prompt-local alias.
    pub alias: String,
    /// The declared capability description.
    pub description: String,
    /// The selected stable live identity.
    pub id: ToolId,
    /// Author override for the model-facing schema description.
    ///
    /// Capability text in [`Self::description`] stays the live H1 bind
    /// string. When set, the executor advertises this instead of the
    /// bound tool's default description.
    pub model_description: Option<String>,
    /// The resolved implementation, attached at bind time.
    pub tool: Arc<dyn Tool>,
    /// Near-duplicate clashes with sibling bindings, recorded at bind time.
    /// Binding records, never fails: a clash errors only when both halves
    /// enter one model-visible scope.
    pub conflicts: Vec<Conflict>,
}

/// Equality is keyed on the binding's data (alias, capability text, stable
/// identity, override, recorded clashes); the attached implementation is a
/// trait object and takes no part in comparison.
impl PartialEq for ToolBinding {
    fn eq(&self, other: &Self) -> bool {
        self.alias == other.alias
            && self.description == other.description
            && self.id == other.id
            && self.model_description == other.model_description
            && self.conflicts == other.conflicts
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
            .field("conflicts", &self.conflicts)
            .finish_non_exhaustive()
    }
}

impl ToolBinding {
    /// Builds a binding for a test double: the identity comes from the tool,
    /// with no override and no recorded clashes.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for `promptforge-core`'s executor
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
            conflicts: Vec::new(),
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

    /// Returns the near-duplicate clashes recorded at bind time.
    #[must_use]
    pub fn conflicts(&self) -> &[Conflict] {
        &self.conflicts
    }
}

/// Inspectable Tool object returned by Lua `tools.bind`.
///
/// Authors read `.name`, `.description`, `.parameters`, `.wire_name`, and
/// `.untrusted`. The object is frozen: model-facing description overrides are
/// positional arguments to `tools.bind` / `tools.always` / `tools.add`, never
/// assignments on this handle. Existing callers that ignore the return value
/// keep working.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LuaToolHandle {
    name: String,
    description: String,
    parameters: Json,
    wire_name: String,
    untrusted: bool,
}

impl LuaToolHandle {
    /// Builds a handle from a bound alias, capability description, and identity.
    ///
    /// Without a live catalog lookup, `wire_name` is the identity's stable
    /// name, `parameters` is an empty object, and `untrusted` is false.
    #[must_use]
    pub(crate) fn from_binding(
        alias: impl Into<String>,
        description: impl Into<String>,
        id: &ToolId,
    ) -> Self {
        Self {
            name: alias.into(),
            description: description.into(),
            parameters: json!({}),
            wire_name: id.name().to_owned(),
            untrusted: false,
        }
    }

    /// Builds a handle from a live tool and its prompt-local binding metadata.
    pub(crate) fn from_live_binding(
        alias: impl Into<String>,
        description: impl Into<String>,
        tool: &dyn Tool,
    ) -> Self {
        Self {
            name: alias.into(),
            description: description.into(),
            parameters: tool.parameters_schema(),
            wire_name: tool.wire_name().to_owned(),
            // Trust is now carried per-call in `ToolOutput`, not a static
            // per-tool flag; the executor wraps untrusted results at dispatch.
            untrusted: false,
        }
    }

    /// Returns the prompt-local alias.
    #[must_use]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

impl UserData for LuaToolHandle {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, this| Ok(this.name.clone()));
        fields.add_field_method_get("description", |_, this| Ok(this.description.clone()));
        fields.add_field_method_get("parameters", |lua, this| lua.to_value(&this.parameters));
        fields.add_field_method_get("wire_name", |_, this| Ok(this.wire_name.clone()));
        fields.add_field_method_get("untrusted", |_, this| Ok(this.untrusted));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(
            MetaMethod::NewIndex,
            |_, _, (key, _): (String, Value)| -> mlua::Result<()> {
                Err(mlua::Error::external(format!(
                    "Tool objects are frozen: cannot assign field {key:?}"
                )))
            },
        );
    }
}

/// One fanout arm result exposed to Lua as a structured object.
///
/// Authors read `.text`, `.ok`, `.item`, and `.exhausted`. `__tostring` returns
/// `.text` so `tostring` and a tostring-coercing `table.concat` keep working.
/// `.item` carries the arm's member value back as a Lua value via the same
/// serde bridge that seeds `var`.
#[derive(Debug, Clone, PartialEq)]
pub struct LuaFanoutResult {
    text: String,
    ok: bool,
    item: Json,
    exhausted: bool,
}

impl LuaFanoutResult {
    /// Builds a successful arm result.
    #[must_use]
    pub fn success(item: impl Into<Json>, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ok: true,
            item: item.into(),
            exhausted: false,
        }
    }

    /// Builds a soft-degraded arm result after tool-loop exhaustion.
    #[must_use]
    pub fn exhausted_stub(item: impl Into<Json>, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ok: false,
            item: item.into(),
            exhausted: true,
        }
    }
}

impl UserData for LuaFanoutResult {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("text", |_, this| Ok(this.text.clone()));
        fields.add_field_method_get("ok", |_, this| Ok(this.ok));
        fields.add_field_method_get("item", |lua, this| lua.to_value(&this.item));
        fields.add_field_method_get("exhausted", |_, this| Ok(this.exhausted));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.text.clone()));
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

/// Resolves an `execute` / `jump` target from a heading string.
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

/// The run's tool set: the prompt-level bindings produced by live H1
/// execution plus the prompt-wide `always` aliases.
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
    /// `#[doc(hidden)]`: a cross-crate seam for `promptforge-core`'s executor
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
/// The run context shares the set as `Arc<dyn ToolView>`; the live H1 pass
/// writes through its own concrete `Arc<Mutex<ToolSet>>` handle, and once
/// that VM is dropped no write handle remains. The trait exposes no
/// mutation, so post-H1 frozenness is structural. Every method locks
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
