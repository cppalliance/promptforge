//! Sandboxed Lua execution for a section's Lua block.
//!
//! A section's Lua chunk runs in a fresh, restricted `mlua` VM: only the
//! `string`, `table`, and `math` standard libraries plus the safe base
//! functions are available; the raw input `args` string and the runtime `sys`
//! table are exposed; a writable `var` table is provided for the block to
//! populate; an always-on `store` table gives the block the run's virtual
//! files; and an instruction-count hook aborts a runaway block.
//! Direct `print` and `warn` are unavailable. During each executable Lua phase,
//! a borrowed `log(message)` callback accepts one bounded, single-line UTF-8
//! string and reports it through the run's [`Observer`] as `Lua: <message>`.
//! The callback expires at the end of that phase and is never retained across
//! a model await.
//!
//! The chunk's top-level return value becomes the section's result (the finish
//! case of the exit rule). The `var` table is read back afterward as JSON for
//! prose substitution.
//!
//! The `store` table is a deterministic host capability (like `var`), always
//! present and independent of tool scoping. Its methods are backed by the
//! run-scoped [`StoreRef`] handle threaded in from the executor, so every section
//! in a run shares one set of virtual files even though contexts clear on each
//! transition. A failed store op raises a Lua error, which surfaces from
//! [`run_chunk`] as [`Error::Lua`].

// These imports are re-exported `pub(crate)` so the `lua` child modules can pull
// the full shared surface with a single `use super::*;`. The `lua` module itself
// is `pub(crate)`, so none of these re-exports widen the crate's public API.
pub(crate) use std::collections::BTreeMap;
pub(crate) use std::num::NonZeroU32;
pub(crate) use std::sync::Arc;
pub(crate) use std::sync::Mutex;
pub(crate) use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

pub(crate) use mlua::{
    Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, MetaMethod, MultiValue, Scope, StdLib,
    UserData, UserDataFields, UserDataMethods, Value, Variadic, VmState,
};
pub(crate) use serde_json::Value as Json;
pub(crate) use serde_json::json;

pub(crate) use crate::lua_models::{LuaModelHandle, ModelInferHook};
pub(crate) use crate::lua_models::{
    ModelBindingState, ModelRuntime, close_model_scope, install_h2_models, install_live_models,
};
pub(crate) use crate::model::{ModelBinding, ModelBindings, ModelResolver};
pub(crate) use crate::observe::{Observation, Observer, detail};
pub(crate) use crate::resolve::RuntimeResolution;
pub(crate) use crate::store::StoreRef;
pub(crate) use crate::tools::{ToolId, ToolRegistry};
pub(crate) use crate::{Error, Result};

/// How many instructions between hook firings.
const HOOK_INTERVAL: u32 = 10_000;
/// Maximum number of hook firings before a block is aborted (~1e7 instructions).
const HOOK_BUDGET: u64 = 1_000;
/// Maximum number of Unicode scalar values accepted by `log`.
const LUA_LOG_CHARACTER_LIMIT: usize = 256;
/// Default per-VM Lua heap ceiling, matching [`crate::execute::RunLimits`].
const DEFAULT_LUA_MEMORY_BYTES: usize = 64 * 1024 * 1024;
/// Default per-VM `log()` event budget, matching [`crate::execute::RunLimits`].
const DEFAULT_LUA_LOG_EVENTS: u32 = 1024;

/// Cumulative `log()` byte ceiling derived from the event budget.
///
/// Bounds total log volume (bytes) even when each event is under the per-event
/// character ceiling. Derived as `events * LUA_LOG_CHARACTER_LIMIT` so it scales
/// with the configured event budget.
fn default_log_byte_budget(log_events: u32) -> usize {
    (log_events as usize).saturating_mul(LUA_LOG_CHARACTER_LIMIT)
}

/// Resolves one plain-English capability description to one stable live tool.
///
/// This is the deterministic seam used by live H1 resolution. It keeps core
/// independent of any concrete picker implementation while allowing a caller
/// to supply a fixed resolver in tests.
pub(crate) trait ToolResolver: Send + Sync {
    /// Resolves `description` to a stable tool identity.
    ///
    /// # Errors
    /// Returns a core error when the capability cannot be resolved uniquely.
    fn resolve(&self, description: &str) -> Result<ToolId>;
}

impl<F> ToolResolver for F
where
    F: Fn(&str) -> Result<ToolId> + Send + Sync,
{
    fn resolve(&self, description: &str) -> Result<ToolId> {
        self(description)
    }
}

/// One prompt-local alias bound to one stable live tool identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolBinding {
    alias: String,
    description: String,
    id: ToolId,
    /// Author override for the model-facing schema description.
    ///
    /// Capability text in [`Self::description`] stays the live H1 need
    /// string. When set, [`crate::execute`] advertises this instead of the
    /// registry tool's default description.
    model_description: Option<String>,
}

impl ToolBinding {
    #[cfg(test)]
    pub(crate) fn for_test(alias: &str, description: &str, id: ToolId) -> Self {
        Self {
            alias: alias.to_owned(),
            description: description.to_owned(),
            id,
            model_description: None,
        }
    }

    /// Returns the exact prompt-local alias.
    #[must_use]
    pub(crate) fn alias(&self) -> &str {
        &self.alias
    }

    /// Returns the declared capability description.
    #[must_use]
    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    /// Returns the selected stable live identity.
    #[must_use]
    pub(crate) fn id(&self) -> &ToolId {
        &self.id
    }

    /// Returns the author override for the model-facing description, if any.
    #[must_use]
    pub(crate) fn model_description(&self) -> Option<&str> {
        self.model_description.as_deref()
    }
}

/// Inspectable Tool object returned by Lua `tools.need`.
///
/// Authors read `.name`, `.description`, `.parameters`, `.wire_name`, and
/// `.untrusted`. `.description` is mutable: assigning it before `tools.add`
/// overrides the model-facing schema text. Existing callers that ignore the
/// return value keep working.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LuaToolHandle {
    name: String,
    description: String,
    description_overridden: bool,
    parameters: Json,
    wire_name: String,
    untrusted: bool,
}

impl LuaToolHandle {
    /// Builds a handle from a bound alias, capability description, and identity.
    ///
    /// Without a live registry lookup, `wire_name` is the identity's stable
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
            description_overridden: false,
            parameters: json!({}),
            wire_name: id.name().to_owned(),
            untrusted: false,
        }
    }

    fn from_live_binding(
        alias: impl Into<String>,
        description: impl Into<String>,
        tool: &dyn crate::tools::Tool,
    ) -> Self {
        Self {
            name: alias.into(),
            description: description.into(),
            description_overridden: false,
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

    /// Returns the model-facing description override when the author assigned
    /// `.description` on this handle.
    #[must_use]
    pub(crate) fn model_description_override(&self) -> Option<&str> {
        self.description_overridden
            .then_some(self.description.as_str())
    }
}

impl UserData for LuaToolHandle {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, this| Ok(this.name.clone()));
        fields.add_field_method_get("description", |_, this| Ok(this.description.clone()));
        fields.add_field_method_set("description", |_, this, value: String| {
            this.description = value;
            this.description_overridden = true;
            Ok(())
        });
        fields.add_field_method_get("parameters", |lua, this| lua.to_value(&this.parameters));
        fields.add_field_method_get("wire_name", |_, this| Ok(this.wire_name.clone()));
        fields.add_field_method_get("untrusted", |_, this| Ok(this.untrusted));
    }
}

/// Inspectable Section object from the Lua `tasks` table.
///
/// Authors read `.name` and `.has_prose`, and pass the object to `execute` or
/// `jump` in place of a heading string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LuaSectionHandle {
    name: String,
    heading: String,
    has_prose: bool,
}

impl LuaSectionHandle {
    /// Builds a handle for a top-level H2 section.
    #[must_use]
    pub(crate) fn new(name: impl Into<String>, has_prose: bool) -> Self {
        let name = name.into();
        let heading = format!("## {name}");
        Self {
            name,
            heading,
            has_prose,
        }
    }

    /// Returns the canonical `"## Name"` heading used by `execute` / `jump`.
    #[must_use]
    pub(crate) fn heading(&self) -> &str {
        &self.heading
    }
}

impl UserData for LuaSectionHandle {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, this| Ok(this.name.clone()));
        fields.add_field_method_get("has_prose", |_, this| Ok(this.has_prose));
    }
}

/// One fanout arm result exposed to Lua as a structured object.
///
/// Authors read `.text`, `.ok`, `.item`, and `.exhausted`. `__tostring` returns
/// `.text` so `tostring` and a tostring-coercing `table.concat` keep working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LuaFanoutResult {
    text: String,
    ok: bool,
    item: String,
    exhausted: bool,
}

impl LuaFanoutResult {
    /// Builds a successful arm result.
    #[must_use]
    pub(crate) fn success(item: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ok: true,
            item: item.into(),
            exhausted: false,
        }
    }

    /// Builds a soft-degraded arm result after tool-loop exhaustion.
    #[must_use]
    pub(crate) fn exhausted_stub(item: impl Into<String>, text: impl Into<String>) -> Self {
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
        fields.add_field_method_get("item", |_, this| Ok(this.item.clone()));
        fields.add_field_method_get("exhausted", |_, this| Ok(this.exhausted));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.text.clone()));
    }
}

/// Outcome of a Lua block that may invoke `jump`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LuaBlockResult {
    /// Normal completion with an optional scalar return.
    Returned(Option<String>),
    /// `jump` transferred control to this heading (`## Name`).
    Jump(String),
}

/// Resolves an `execute` / `jump` target from a heading string or Section object.
///
/// # Errors
/// Returns a Lua error when the value is neither a string nor a Section handle.
pub(crate) fn resolve_section_target(value: Value) -> mlua::Result<String> {
    match value {
        Value::String(s) => Ok(s.to_str()?.to_owned()),
        Value::UserData(ud) => {
            let handle = ud.borrow::<LuaSectionHandle>()?;
            Ok(handle.heading().to_owned())
        }
        other => Err(mlua::Error::external(format!(
            "section target must be a string or Section object, got {}",
            other.type_name()
        ))),
    }
}

/// Immutable prompt-level tool bindings produced by live H1 execution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ToolBindings {
    bindings: Vec<ToolBinding>,
    always: Vec<String>,
}

impl ToolBindings {
    #[cfg(test)]
    pub(crate) fn for_test(bindings: Vec<ToolBinding>, always: Vec<String>) -> Self {
        Self { bindings, always }
    }

    /// Returns bindings in declaration order.
    #[must_use]
    pub(crate) fn bindings(&self) -> &[ToolBinding] {
        &self.bindings
    }

    /// Returns prompt-wide aliases in declaration order.
    #[must_use]
    pub(crate) fn always(&self) -> &[String] {
        &self.always
    }

    fn binding(&self, alias: &str) -> Option<&ToolBinding> {
        self.bindings.iter().find(|binding| binding.alias == alias)
    }
}

/// Shared per-VM tool-call counts, pre-seeded at 0 for every in-scope alias.
///
/// The executor increments a count when dispatch is attempted (even if the tool
/// later errors). Lua reads the snapshot through the `tools.calls` table.
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolCallCounts {
    inner: Arc<Mutex<BTreeMap<String, u64>>>,
}

impl ToolCallCounts {
    /// Creates a counts map pre-seeded with 0 for every alias.
    #[must_use]
    pub(crate) fn new(aliases: impl IntoIterator<Item = String>) -> Self {
        let map: BTreeMap<String, u64> = aliases.into_iter().map(|a| (a, 0)).collect();
        Self {
            inner: Arc::new(Mutex::new(map)),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, u64>>> {
        self.inner
            .lock()
            .map_err(|_| Error::Lua("tool call counts mutex was poisoned".to_owned()))
    }

    /// Ensures `alias` is present in the map, seeding it at 0 when new.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the mutex is poisoned.
    pub(crate) fn ensure(&self, alias: &str) -> Result<()> {
        let mut map = self.lock()?;
        map.entry(alias.to_owned()).or_insert(0);
        Ok(())
    }

    /// Increments the count for `alias`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the mutex is poisoned or alias is not in scope.
    pub(crate) fn increment(&self, alias: &str) -> Result<()> {
        let mut map = self.lock()?;
        let count = map.get_mut(alias).ok_or_else(|| {
            Error::Lua(format!(
                "tool call counts: alias {alias:?} was not pre-seeded"
            ))
        })?;
        *count += 1;
        Ok(())
    }

    /// Returns the current count for `alias`, or `None` if not in scope.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the mutex is poisoned.
    pub(crate) fn get(&self, alias: &str) -> Result<Option<u64>> {
        Ok(self.lock()?.get(alias).copied())
    }

    /// Returns a snapshot of all in-scope aliases.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the mutex is poisoned.
    pub(crate) fn aliases(&self) -> Result<Vec<String>> {
        Ok(self.lock()?.keys().cloned().collect())
    }
}

/// A closed H2 tool scope, ordered with prompt-wide aliases first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolScope {
    bindings: Vec<ToolBinding>,
}

impl ToolScope {
    /// Builds a scope from already resolved bindings.
    #[must_use]
    pub(crate) fn from_bindings(bindings: Vec<ToolBinding>) -> Self {
        Self { bindings }
    }

    /// Returns the effective bindings in model-advertisement order.
    #[must_use]
    pub(crate) fn bindings(&self) -> &[ToolBinding] {
        &self.bindings
    }
}

/// Closed H2 tool scope and optional section model selection.
// No `Eq`: the optional model binding carries an `f64` temperature transitively.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ClosedScopes {
    /// Effective tool bindings for this section.
    pub(crate) tools: ToolScope,
    /// Selected model binding from `models.use` or prompt-wide `models.always`.
    pub(crate) model: Option<ModelBinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolPhase {
    H2,
    Closed,
}

#[derive(Debug)]
pub(crate) struct ToolRuntime {
    phase: ToolPhase,
    added: Vec<String>,
    /// Per-alias author overrides for model-facing schema descriptions.
    description_overrides: BTreeMap<String, String>,
    /// Monotonic counter bumped when the effective H2 tool set changes.
    ///
    /// [`crate::execute::ToolBag`] caches schemas/dispatch against this value
    /// so `model:infer` rebuilds only after a real mutation.
    generation: u64,
}

impl ToolRuntime {
    /// Returns the current tool-set generation.
    #[must_use]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

/// Compiled Lua 5.4 source that can be loaded into multiple process-local VMs.
///
/// A program retains its original source for diagnostics and stores bytecode
/// produced once by Lua 5.4. The bytecode is an in-memory implementation detail:
/// it is not a stable or portable serialization format and must not be persisted.
///
/// Compilation does not execute the source. Loading a program (a crate-internal
/// step) creates a function in the supplied VM but likewise does not call it.
///
/// `#[non_exhaustive]` so the crate can evolve the retained representation
/// (fields are already private) without a breaking change before release.
///
/// # Sensitivity (LUA-015)
/// A program retains the author's original prompt Lua source verbatim, and
/// [`source`](Self::source) exposes it. Prompt source can embed
/// author-sensitive material (system instructions, embedded credentials in a
/// poorly written prompt, private policy text), so treat the value returned by
/// [`source`](Self::source) as sensitive: it is a full-fidelity diagnostic
/// accessor, not a value to log at info level, echo to untrusted sinks, or place
/// in a model-facing message. [`location`](Self::location) and
/// [`source_line`](Self::source_line) are safe positional metadata (a chunk name
/// and a line number) and carry no source text. The crate itself never logs the
/// retained source; compilation observations carry only fixed strings.
///
/// # Examples
/// A program is obtained from the parser and exposes its source and position:
/// ```
/// use promptforge_core::observe::NullObserver;
/// use promptforge_core::parser::Prompt;
///
/// let source = concat!(
///     "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n",
///     "# Title\n\nintro\n\n",
///     "## Only\n\n",
///     "```lua\nreturn 1\n```\n",
/// );
/// let prompt = Prompt::parse(source, "doc", &NullObserver)?;
/// let program = prompt.sections()[0]
///     .prologue()
///     .expect("the section has a Lua prologue");
/// assert_eq!(program.source(), "return 1");
/// assert!(program.source_line().get() >= 1);
/// assert!(program.location().contains("Only"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LuaProgram {
    source: String,
    bytecode: Vec<u8>,
    /// Parser location string used as the Lua chunk name (for example
    /// `section \`Web Search\` epilog`).
    location: String,
    /// 1-based line number in the prompt source where this Lua region begins.
    ///
    /// A [`NonZeroU32`] so line zero is unrepresentable. Used together with
    /// chunk-relative line numbers from Lua runtime errors to produce an
    /// absolute prompt-source line: `source_line + chunk_line - 1`.
    source_line: NonZeroU32,
}

impl LuaProgram {
    /// Compiles `source` as Lua 5.4 bytecode without executing it.
    ///
    /// `location` identifies the source region in diagnostics. Compilation
    /// reports contain only fixed strings and never include `source` or
    /// `location`; each carries `execution` unchanged.
    ///
    /// # Errors
    /// Returns [`Error::LuaCompile`] when `source` is not syntactically valid,
    /// retaining the source, location, and Lua diagnostic. Returns
    /// [`Error::Lua`] if the temporary compiler VM cannot be created.
    ///
    /// # Examples
    /// ```text
    /// use mlua::Lua;
    /// use promptforge_core::lua::LuaProgram;
    /// use promptforge_core::observe::NullObserver;
    ///
    /// let program = LuaProgram::compile(
    ///     "return 40 + 2",
    ///     "example prologue",
    ///     1,
    ///     "example-run",
    ///     &NullObserver,
    ///     "Example",
    /// )?;
    /// let lua = Lua::new();
    /// let chunk = program.load(&lua)?;
    /// let answer: i64 = chunk.call(())?;
    /// assert_eq!(answer, 42);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub(crate) fn compile(
        source: &str,
        location: &str,
        source_line: NonZeroU32,
        execution: &str,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Self> {
        observer.observe(execution, section, detail::LUA_COMPILATION_STARTED);

        let lua = match Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH,
            LuaOptions::default(),
        ) {
            Ok(lua) => lua,
            Err(error) => {
                observer.observe(execution, section, detail::LUA_COMPILATION_FAILED);
                return Err(Error::Lua(error.to_string()));
            }
        };

        let function = match lua.load(source).set_name(location).into_function() {
            Ok(function) => function,
            Err(error) => {
                observer.observe(execution, section, detail::LUA_COMPILATION_FAILED);
                return Err(Error::LuaCompile {
                    location: location.to_owned(),
                    source_line: source_line.get(),
                    lua_source: source.to_owned(),
                    message: error.to_string(),
                });
            }
        };
        // Keep debug info so runtime errors report the chunk name and line
        // (`dump(true)` strips them and leaves `?:` in the traceback).
        let bytecode = function.dump(false);

        observer.observe(execution, section, detail::LUA_COMPILATION_SUCCEEDED);
        Ok(Self {
            source: source.to_owned(),
            bytecode,
            location: location.to_owned(),
            source_line,
        })
    }

    /// Returns the original Lua source retained for diagnostics.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the 1-based prompt-source line where this Lua region begins.
    #[must_use]
    pub fn source_line(&self) -> NonZeroU32 {
        self.source_line
    }

    /// Loads the compiled function into `lua` without executing it.
    ///
    /// The bytecode is loaded only into a VM in the same process and is never
    /// exposed as a persistence format.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the VM rejects the internally compiled
    /// bytecode.
    pub(crate) fn load(&self, lua: &Lua) -> Result<Function> {
        lua.load(self.bytecode.as_slice())
            .into_function()
            .map_err(|error| Error::Lua(error.to_string()))
    }

    /// Maps a Lua runtime error to an [`Error::Lua`] with the chunk-relative
    /// line rewritten to an absolute prompt-source line.
    ///
    /// Lua errors look like `[string "chunk name"]:N: message`. This method
    /// rewrites only this program's chunk-relative `:N:` to the absolute
    /// prompt line `source_line + N - 1`, and prefixes a clear
    /// `{location}:{absolute}:` tag so hosts can show `file:line` without
    /// parsing Lua's `[string ...]` form. Nested errors from other chunks
    /// (for example a fanout arm) are left unchanged.
    pub(crate) fn map_runtime_error(&self, error: &mlua::Error) -> Error {
        // A block aborted by the cancellation hook surfaces as an interruption,
        // not a Lua authoring error.
        if crate::cancel::is_cancelled() {
            return Error::Interrupted;
        }
        let raw = error.to_string();
        // A host-quota refusal is a stable typed error, not an authoring error.
        if let Some(resource) = quota_resource(&raw) {
            return Error::LuaQuota { resource };
        }
        let mapped = map_chunk_line_to_absolute(&raw, self.source_line, self.location());
        Error::Lua(mapped)
    }

    /// Chunk name recorded at compile time (parser location string).
    #[must_use]
    pub fn location(&self) -> &str {
        &self.location
    }
}

/// Maps a raw Lua error string to the exhausted host-quota resource, if any.
///
/// Recognizes the stable quota messages our host callbacks emit so a refusal
/// becomes the typed [`Error::LuaQuota`] instead of an opaque `Lua(String)`.
fn quota_resource(raw: &str) -> Option<&'static str> {
    use crate::error::lua_quota;
    if raw.contains(lua_quota::LOG_EVENT) {
        Some("log event")
    } else if raw.contains(lua_quota::LOG_BYTE) {
        Some("log byte")
    } else if raw.contains(lua_quota::INSTRUCTION) {
        Some("instruction")
    } else {
        None
    }
}

/// Rewrites chunk-relative line numbers for one named chunk to absolute
/// prompt-source lines.
///
/// Only `[string "{location}"]:N:` occurrences are rewritten, so a parent
/// prologue that surfaces a fanout child's already-mapped error does not
/// corrupt the child's absolute line. When the pattern is absent, the message
/// passes through unchanged except for a leading `{location}:` tag when an
/// absolute line can still be inferred. A chunk line whose absolute mapping
/// would overflow `u32` is left as its original digits (the finding's
/// "return the original diagnostic on overflow").
fn map_chunk_line_to_absolute(message: &str, source_line: NonZeroU32, location: &str) -> String {
    if location.is_empty() {
        return message.to_owned();
    }
    let marker = format!("[string \"{location}\"]:");
    let mut result = String::with_capacity(message.len() + 64);
    let mut rest = message;
    let mut first_absolute: Option<u32> = None;
    while let Some(start) = rest.find(&marker) {
        result.push_str(&rest[..start]);
        result.push_str(&marker);
        let after = &rest[start + marker.len()..];
        let digit_end = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        if digit_end == 0 {
            rest = after;
            continue;
        }
        if let Ok(chunk_line) = after[..digit_end].parse::<u32>() {
            // absolute = source_line + chunk_line - 1, with overflow guarded so
            // a pathological line count cannot wrap into a wrong number.
            let absolute = source_line
                .get()
                .checked_add(chunk_line)
                .and_then(|sum| sum.checked_sub(1));
            match absolute {
                Some(absolute) => {
                    if first_absolute.is_none() {
                        first_absolute = Some(absolute);
                    }
                    result.push_str(&absolute.to_string());
                }
                None => result.push_str(&after[..digit_end]),
            }
            rest = &after[digit_end..];
        } else {
            rest = after;
        }
    }
    result.push_str(rest);

    if let Some(absolute) = first_absolute {
        // Leading tag hosts can show next to the file name: `briefer.md:51: ...`
        format!("{location}:{absolute}: {result}")
    } else {
        result
    }
}

mod hardening;
pub(crate) use hardening::*;
mod sys;
pub(crate) use sys::*;
mod host;
pub(crate) use host::*;
mod tools_bridge;
pub(crate) use tools_bridge::*;
mod vm;
pub(crate) use vm::*;
mod live;
pub(crate) use live::*;
