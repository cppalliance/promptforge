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

#[derive(Debug, Default)]
struct BindingState {
    bindings: Vec<ToolBinding>,
    always: Vec<String>,
    callback_error: Option<Error>,
}

/// Run-scoped accumulator populated by live H1 capability calls.
///
/// The producer is installed into one H1 VM. Every executed `tools.need`,
/// `models.need`, and `models.always` call resolves immediately, while skipped
/// Lua branches produce no binding.
#[derive(Debug, Clone, Default)]
pub(crate) struct LiveBindingProducer {
    tools: Arc<Mutex<BindingState>>,
    models: Arc<Mutex<ModelBindingState>>,
}

impl LiveBindingProducer {
    /// Installs live tool and model tables into `lua` for the lifetime of
    /// `scope`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when either table cannot be installed.
    pub(crate) fn install<'scope, 'env: 'scope, 'tools: 'env>(
        &self,
        lua: &'env Lua,
        scope: &'scope mlua::Scope<'scope, 'env>,
        tool_resolver: &'env dyn ToolResolver,
        registry: &'env ToolRegistry<'tools>,
        model_resolver: &'env dyn ModelResolver,
    ) -> Result<()> {
        install_live_tools(lua, scope, tool_resolver, registry, &self.tools)?;
        install_live_models(lua, scope, model_resolver, &self.models)
    }

    /// Returns the first concrete resolver error captured by a Lua callback.
    ///
    /// This lets the H1 executor preserve typed resolution errors instead of
    /// replacing them with mlua's callback wrapper.
    pub(crate) fn take_callback_error(&self) -> Result<Option<Error>> {
        let tool_error = self
            .tools
            .lock()
            .map_err(|_| Error::Lua("tool binding recorder was poisoned".to_owned()))?
            .callback_error
            .take();
        let model_error = self
            .models
            .lock()
            .map_err(|_| Error::Lua("model binding recorder was poisoned".to_owned()))?
            .callback_error
            .take();
        Ok(tool_error.or(model_error))
    }

    /// Snapshots all bindings resolved by the live H1 execution so far.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if either recorder mutex is poisoned.
    pub(crate) fn bindings(&self) -> Result<(ToolBindings, ModelBindings)> {
        let tools = self
            .tools
            .lock()
            .map_err(|_| Error::Lua("tool binding recorder was poisoned".to_owned()))?;
        let models = self
            .models
            .lock()
            .map_err(|_| Error::Lua("model binding recorder was poisoned".to_owned()))?;
        Ok((
            ToolBindings {
                bindings: tools.bindings.clone(),
                always: tools.always.clone(),
            },
            ModelBindings::from_parts(models.bindings.clone(), models.always.clone()),
        ))
    }
}

/// Installs live H1 tool resolution into an existing Lua VM.
///
/// `tools.need` consults `resolver` at the point Lua executes the call, verifies
/// the selected identity against `registry`, records the frozen binding, and
/// returns an inspectable Tool object populated from the live registry entry.
///
/// # Errors
/// Returns [`Error::Lua`] when the Lua table cannot be installed.
#[expect(
    clippy::too_many_lines,
    reason = "one scoped table keeps its callbacks and shared recorder together"
)]
fn install_live_tools<'scope, 'env: 'scope, 'tools: 'env>(
    lua: &'env Lua,
    scope: &'scope mlua::Scope<'scope, 'env>,
    resolver: &'env dyn ToolResolver,
    registry: &'env ToolRegistry<'tools>,
    state: &Arc<Mutex<BindingState>>,
) -> Result<()> {
    let tools = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;

    let needs = Arc::clone(state);
    let need = scope
        .create_function(
            move |_, (alias, description): (String, String)| -> mlua::Result<LuaToolHandle> {
                validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
                {
                    let mut bindings = needs
                        .lock()
                        .map_err(|_| mlua::Error::external("tool binding recorder was poisoned"))?;
                    if bindings
                        .bindings
                        .iter()
                        .any(|binding| binding.alias == alias)
                    {
                        let error = Error::DuplicateAlias {
                            alias: alias.clone(),
                        };
                        if bindings.callback_error.is_none() {
                            bindings.callback_error = Some(error);
                        }
                        return Err(mlua::Error::external("duplicate tool alias"));
                    }
                }
                let id = match resolver.resolve(&description) {
                    Ok(id) => id,
                    Err(error) => {
                        let mut bindings = needs.lock().map_err(|_| {
                            mlua::Error::external("tool binding recorder was poisoned")
                        })?;
                        if bindings.callback_error.is_none() {
                            bindings.callback_error = Some(error);
                        }
                        return Err(mlua::Error::external("tool capability resolution failed"));
                    }
                };
                let Some(tool) = registry.get(&id) else {
                    let error = Error::PickedToolNotLive {
                        alias: alias.clone(),
                        id,
                    };
                    let mut bindings = needs
                        .lock()
                        .map_err(|_| mlua::Error::external("tool binding recorder was poisoned"))?;
                    if bindings.callback_error.is_none() {
                        bindings.callback_error = Some(error);
                    }
                    return Err(mlua::Error::external("picked tool is not live"));
                };
                let handle = LuaToolHandle::from_live_binding(&alias, &description, tool);
                let mut bindings = needs
                    .lock()
                    .map_err(|_| mlua::Error::external("tool binding recorder was poisoned"))?;
                if let Some(first) = bindings
                    .bindings
                    .iter()
                    .find(|binding| binding.id == id)
                    .map(|binding| binding.alias.clone())
                {
                    let error = Error::ToolIdSelectedTwice {
                        id,
                        first_alias: first,
                        second_alias: alias,
                    };
                    if bindings.callback_error.is_none() {
                        bindings.callback_error = Some(error);
                    }
                    return Err(mlua::Error::external(
                        "tool identity was selected more than once",
                    ));
                }
                bindings.bindings.push(ToolBinding {
                    alias: alias.clone(),
                    description: description.clone(),
                    id,
                    model_description: None,
                });
                Ok(handle)
            },
        )
        .map_err(|error| Error::Lua(error.to_string()))?;
    tools
        .set("need", need)
        .map_err(|error| Error::Lua(error.to_string()))?;

    let prompt_wide = Arc::clone(state);
    let always = scope
        .create_function(move |_, alias: String| -> mlua::Result<()> {
            validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
            let mut bindings = prompt_wide
                .lock()
                .map_err(|_| mlua::Error::external("tool binding recorder was poisoned"))?;
            if !bindings
                .bindings
                .iter()
                .any(|binding| binding.alias == alias)
            {
                return Err(mlua::Error::external(format!(
                    "tools.always alias {alias:?} was not declared by tools.need"
                )));
            }
            if bindings.always.iter().any(|existing| existing == &alias) {
                return Err(mlua::Error::external(format!(
                    "tools.always alias {alias:?} was recorded more than once"
                )));
            }
            bindings.always.push(alias);
            Ok(())
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    tools
        .set("always", always)
        .map_err(|error| Error::Lua(error.to_string()))?;

    let add = scope
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "tools.add is only available during H2 recording",
            ))
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    tools
        .set("add", add)
        .map_err(|error| Error::Lua(error.to_string()))?;
    lua.globals()
        .raw_set("tools", tools)
        .map_err(|error| Error::Lua(error.to_string()))
}

fn validate_alias(alias: &str) -> Result<()> {
    let bytes = alias.as_bytes();
    let valid = (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphabetic()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(Error::Lua(format!(
            "invalid tool alias {alias:?}: expected [A-Za-z][A-Za-z0-9_-]{{0,63}}"
        )))
    }
}

/// One hardened, isolated Lua VM for a section's complete lifecycle.
///
/// The VM owns one Lua environment from construction until drop. An optional
/// shared program runs before host values are installed, then prologue and
/// epilog programs loaded with [`run_prologue`](Self::run_prologue) and
/// [`run_epilog`](Self::run_epilog) see that same environment.
/// [`bind_reply`](Self::bind_reply) inserts the model reply into it between
/// those phases. A single instruction counter covers every program run by this
/// VM, so splitting work across lifecycle phases cannot reset the budget.
///
/// `SectionVm` deliberately does not expose its underlying [`Lua`]. This keeps
/// hardening, host injection, instruction accounting, and report delivery on
/// the one owned path. Each section must receive a new instance; dropping it
/// destroys all Lua memory belonging to that section. Once Lua allocation
/// succeeds, construction, shared-load, and captured-binding failures cross
/// the same explicit observed teardown boundary as later lifecycle failures.
///
/// # Examples
/// ```text
/// use promptforge_core::lua::SectionVm;
/// use promptforge_core::observe::NullObserver;
///
/// let vm = SectionVm::new(None, "example-run", &NullObserver, "Example")?;
/// vm.teardown(&NullObserver, "Example");
/// # Ok::<(), promptforge_core::Error>(())
/// ```
#[derive(Debug)]
pub(crate) struct SectionVm {
    execution: String,
    lua: Lua,
    bound_tools: ToolBindings,
    bound_models: ModelBindings,
    tool_runtime: Arc<Mutex<ToolRuntime>>,
    model_runtime: Arc<Mutex<ModelRuntime>>,
    /// Shared with [`crate::execute::InferContext`] so `model:infer` and the
    /// prose tool loop increment the same `tools.calls` counters.
    counts_slot: Arc<Mutex<Option<ToolCallCounts>>>,
    /// Set by Lua `jump` before it aborts the current chunk.
    jump_slot: Arc<Mutex<Option<String>>>,
    /// Live sealed `sys` JSON shared with `model:infer` for finish-reason updates.
    sys_live: Arc<Mutex<Option<Json>>>,
    store: Option<StoreRef>,
    host_injected: bool,
    /// Remaining `log()` events this VM may emit before the budget is exhausted.
    log_budget: Arc<AtomicU32>,
    /// Remaining cumulative `log()` message bytes this VM may emit. Bounds total
    /// log volume even when each event is under the per-event ceilings.
    log_byte_budget: Arc<AtomicUsize>,
}

impl SectionVm {
    /// Creates a hardened section VM and optionally executes a shared program.
    ///
    /// The shared program runs before `args`, `sys`, `var`, `tools`, `store`,
    /// and `reply` are installed. This delayed injection prevents shared code
    /// from retaining a host value before section execution begins. The VM
    /// retains `execution` for every later lifecycle report. Shared execution
    /// receives a phase-local `log(message)` callback; direct `print` is
    /// unavailable.
    ///
    /// The VM carries no frozen tool bindings, so the validating `tools.add`
    /// installed by [`inject_host`](Self::inject_host) rejects every alias as
    /// undeclared: a prompt without `tools.need` declarations cannot scope
    /// tools.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the VM cannot be built or hardened, or if the
    /// shared program fails or returns a non-scalar value.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_core::lua::{LuaProgram, SectionVm};
    /// use promptforge_core::observe::NullObserver;
    ///
    /// let shared = LuaProgram::compile(
    ///     "function decorate(s) return '<' .. s .. '>' end",
    ///     "shared",
    ///     1,
    ///     "example-run",
    ///     &NullObserver,
    ///     "Example",
    /// )?;
    /// let vm = SectionVm::new(Some(&shared), "example-run", &NullObserver, "Example")?;
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn new(
        shared: Option<&LuaProgram>,
        execution: &str,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Self> {
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH,
            LuaOptions::default(),
        )
        .map_err(|error| Error::Lua(error.to_string()))?;
        // Bound the VM heap by default; `apply_lua_limits` may tighten or relax
        // it to the caller's `RunLimits`. A safe non-env default keeps every VM
        // bounded even when the run installs no explicit limits.
        lua.set_memory_limit(DEFAULT_LUA_MEMORY_BYTES)
            .map_err(|error| Error::Lua(error.to_string()))?;
        let vm = Self {
            execution: execution.to_owned(),
            lua,
            bound_tools: ToolBindings::default(),
            bound_models: ModelBindings::default(),
            tool_runtime: Arc::new(Mutex::new(ToolRuntime {
                phase: ToolPhase::H2,
                added: Vec::new(),
                description_overrides: BTreeMap::new(),
                generation: 0,
            })),
            model_runtime: Arc::new(Mutex::new(ModelRuntime::new())),
            counts_slot: Arc::new(Mutex::new(None)),
            jump_slot: Arc::new(Mutex::new(None)),
            sys_live: Arc::new(Mutex::new(None)),
            store: None,
            host_injected: false,
            log_budget: Arc::new(AtomicU32::new(DEFAULT_LUA_LOG_EVENTS)),
            log_byte_budget: Arc::new(AtomicUsize::new(default_log_byte_budget(
                DEFAULT_LUA_LOG_EVENTS,
            ))),
        };
        if let Err(error) = harden(&vm.lua) {
            return vm.construction_failed(error, observer, section);
        }
        install_instruction_budget(&vm.lua);
        if let Some(program) = shared {
            observer.observe(execution, section, detail::LUA_SHARED_LOAD_STARTED);
            match vm.run_loaded_with_log(program, observer, section) {
                Ok(_) => observer.observe(execution, section, detail::LUA_SHARED_LOAD_SUCCEEDED),
                Err(error) => {
                    observer.observe(execution, section, detail::LUA_SHARED_LOAD_FAILED);
                    return vm.construction_failed(error, observer, section);
                }
            }
        }
        Ok(vm)
    }

    /// Creates a section VM, loads its shared library, then installs captured bindings.
    ///
    /// The shared program runs before any host API, including `log`, or captured
    /// binding exists.
    /// Its functions may refer to those globals because Lua resolves globals
    /// when a function is called. Rust installs each captured Tool and Model
    /// object directly after the shared load, without replaying H1 code.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if VM construction, shared-library execution, or
    /// captured-binding installation fails.
    pub(crate) fn new_for_section(
        replay: Option<&LuaProgram>,
        tools: &ToolBindings,
        models: &ModelBindings,
        execution: &str,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Self> {
        let mut vm = Self::new(None, execution, observer, section)?;
        if let Some(program) = replay {
            observer.observe(execution, section, detail::LUA_SHARED_LOAD_STARTED);
            match vm.run_loaded_without_host(program) {
                Ok(_) => observer.observe(execution, section, detail::LUA_SHARED_LOAD_SUCCEEDED),
                Err(error) => {
                    observer.observe(execution, section, detail::LUA_SHARED_LOAD_FAILED);
                    return vm.construction_failed(error, observer, section);
                }
            }
        }
        vm.bound_tools = tools.clone();
        vm.bound_models = models.clone();
        if let Err(error) = vm.install_captured_bindings() {
            return vm.construction_failed(error, observer, section);
        }
        Ok(vm)
    }

    fn install_captured_bindings(&self) -> Result<()> {
        let globals = self.lua.globals();
        for binding in self.bound_tools.bindings() {
            let handle =
                LuaToolHandle::from_binding(binding.alias(), binding.description(), binding.id());
            let userdata = self
                .lua
                .create_userdata(handle)
                .map_err(|error| Error::Lua(error.to_string()))?;
            globals
                .raw_set(binding.alias(), userdata)
                .map_err(|error| Error::Lua(error.to_string()))?;
        }
        for binding in self.bound_models.bindings() {
            let userdata = self
                .lua
                .create_userdata(LuaModelHandle::from_binding(binding))
                .map_err(|error| Error::Lua(error.to_string()))?;
            globals
                .raw_set(binding.alias(), userdata)
                .map_err(|error| Error::Lua(error.to_string()))?;
        }
        Ok(())
    }

    /// Installs the section's host values after the shared program has run.
    ///
    /// This operation may be called exactly once. The store callbacks own a
    /// clone of the run-scoped store. StoreRef functions are installed with
    /// phase-local borrowed observation context by
    /// [`run_prologue`](Self::run_prologue) and [`run_epilog`](Self::run_epilog),
    /// so no observer reference is retained while the VM waits for a model reply.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values cannot be bridged or if host
    /// values were already injected.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_core::lua::SectionVm;
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::StoreRef;
    ///
    /// let mut vm = SectionVm::new(None, "example-run", &NullObserver, "Example")?;
    /// vm.inject_host("input", &serde_json::json!({ "id": 1 }), &StoreRef::memory(), None)?;
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn inject_host(
        &mut self,
        args: &str,
        sys: &Json,
        store: &StoreRef,
        last_reply: Option<&str>,
    ) -> Result<()> {
        self.inject_host_with_var(args, sys, store, last_reply, None)
    }

    /// Installs host values while seeding `var` from an earlier VM.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values cannot be bridged or were already
    /// injected.
    pub(crate) fn inject_host_with_var(
        &mut self,
        args: &str,
        sys: &Json,
        store: &StoreRef,
        last_reply: Option<&str>,
        initial_var: Option<&Json>,
    ) -> Result<()> {
        if self.host_injected {
            return Err(Error::Lua(
                "section VM host values were already injected".to_owned(),
            ));
        }

        let globals = self.lua.globals();
        globals
            .raw_set("args", args)
            .map_err(|error| Error::Lua(error.to_string()))?;
        let sys_table = seal_sys(&self.lua, sys)?;
        globals
            .raw_set("sys", sys_table)
            .map_err(|error| Error::Lua(error.to_string()))?;
        {
            let mut live = self
                .sys_live
                .lock()
                .map_err(|_| Error::Lua("sys live slot was poisoned".to_owned()))?;
            *live = Some(sys.clone());
        }
        let var = match initial_var {
            Some(value) => self
                .lua
                .to_value(value)
                .map_err(|error| Error::Lua(error.to_string()))?,
            None => Value::Table(
                self.lua
                    .create_table()
                    .map_err(|error| Error::Lua(error.to_string()))?,
            ),
        };
        globals
            .raw_set("var", var)
            .map_err(|error| Error::Lua(error.to_string()))?;
        install_h2_tools(&self.lua, &globals, &self.bound_tools, &self.tool_runtime)?;
        install_h2_models(&self.lua, &globals, &self.bound_models, &self.model_runtime)?;
        let reply_value = match last_reply {
            Some(text) => Value::String(
                self.lua
                    .create_string(text)
                    .map_err(|error| Error::Lua(error.to_string()))?,
            ),
            None => Value::Nil,
        };
        globals
            .raw_set("reply", reply_value)
            .map_err(|error| Error::Lua(error.to_string()))?;
        self.store = Some(store.clone());
        self.host_injected = true;
        Ok(())
    }

    /// Executes one live H1 Lua block with call-time capability resolution.
    ///
    /// Resolver callbacks are scoped to this block and reinstalled for each
    /// later H1 Lua block. Resolved Tool and Model objects remain ordinary Lua
    /// values in the VM.
    ///
    /// # Errors
    /// Returns typed capability errors captured by the runtime resolver, or the
    /// underlying Lua execution error.
    pub(crate) fn run_live_h1_block(
        &self,
        program: &LuaProgram,
        resolution: &RuntimeResolution<'_, '_>,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        let result = self.lua.scope(|scope| {
            resolution
                .install(&self.lua, scope)
                .map_err(|error| mlua::Error::external(error.to_string()))?;
            self.run_prologue(program, observer, section)
                .map_err(|error| mlua::Error::external(error.to_string()))
        });
        match result {
            Ok(value) => Ok(value),
            Err(error) => match resolution.take_callback_error()? {
                Some(error) => Err(error),
                None => Err(Error::Lua(error.to_string())),
            },
        }
    }

    /// Replaces the sealed Lua `sys` global after scope close.
    ///
    /// Host injection must have run first. Used to expose `sys.model` once the
    /// section's model binding is fixed.
    pub(crate) fn re_seal_sys(&self, sys: &Json) -> Result<()> {
        if !self.host_injected {
            return Err(Error::Lua(
                "section VM host values were not injected".to_owned(),
            ));
        }
        let globals = self.lua.globals();
        let sys_table = seal_sys(&self.lua, sys)?;
        globals
            .raw_set("sys", sys_table)
            .map_err(|error| Error::Lua(error.to_string()))?;
        let mut live = self
            .sys_live
            .lock()
            .map_err(|_| Error::Lua("sys live slot was poisoned".to_owned()))?;
        *live = Some(sys.clone());
        Ok(())
    }

    /// Shared live `sys` JSON for `model:infer` finish-reason updates.
    pub(crate) fn sys_live_handle(&self) -> Arc<Mutex<Option<Json>>> {
        Arc::clone(&self.sys_live)
    }

    /// Snapshot of the live sealed `sys` JSON, or `fallback` when unset.
    ///
    /// Distinguishes the two non-value states rather than collapsing both to
    /// `fallback`: an *unset* live slot (before any [`Self::re_seal_sys`]) is a
    /// legitimate state and yields `Ok(fallback)`, while a *poisoned* lock is a
    /// real failure and yields [`Error::Lua`] instead of silently masquerading
    /// as the fallback.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the live `sys` mutex is poisoned.
    pub(crate) fn current_sys(&self, fallback: &Json) -> Result<Json> {
        let guard = self
            .sys_live
            .lock()
            .map_err(|_| Error::Lua("sys live slot was poisoned".to_owned()))?;
        Ok(guard.clone().unwrap_or_else(|| fallback.clone()))
    }

    /// Executes a compiled prologue in this VM's persistent environment.
    ///
    /// StoreRef-operation reports recorded by host callbacks are delivered in
    /// operation order before this method returns, including when execution
    /// fails. A nil or absent top-level return produces `None`; strings,
    /// integers, numbers, and booleans produce their scalar string form.
    /// `log(message)` is available only for this call and reports under
    /// `execution` and `section`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected, execution
    /// fails, the shared instruction budget is exhausted, or the program
    /// returns a non-scalar value.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_core::lua::{LuaProgram, SectionVm};
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::StoreRef;
    ///
    /// let prologue = LuaProgram::compile(
    ///     "var.answer = 42",
    ///     "prologue",
    ///     1,
    ///     "example-run",
    ///     &NullObserver,
    ///     "Example",
    /// )?;
    /// let mut vm = SectionVm::new(None, "example-run", &NullObserver, "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &StoreRef::memory(), None)?;
    /// assert_eq!(vm.run_prologue(&prologue, &NullObserver, "Example")?, None);
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn run_prologue(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        observer.observe(&self.execution, section, detail::LUA_PROLOGUE_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(&self.execution, section, detail::LUA_PROLOGUE_FAILED);
            return Err(error);
        }
        let result = self.run_loaded_with_host(program, observer, section);
        observer.observe(
            &self.execution,
            section,
            if result.is_ok() {
                detail::LUA_PROLOGUE_SUCCEEDED
            } else {
                detail::LUA_PROLOGUE_FAILED
            },
        );
        result
    }

    /// Executes a compiled prologue with `tasks`, `execute`, `jump`, and optional `fanout`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected or
    /// execution fails.
    pub(crate) fn run_prologue_with_control<E, F>(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
        tasks: &[LuaSectionHandle],
        execute_callback: Option<&E>,
        fanout_callback: Option<&F>,
    ) -> Result<LuaBlockResult>
    where
        E: Fn(Value, Option<String>) -> std::result::Result<String, String>,
        F: Fn(String, String) -> std::result::Result<Vec<LuaFanoutResult>, String>,
    {
        observer.observe(&self.execution, section, detail::LUA_PROLOGUE_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(&self.execution, section, detail::LUA_PROLOGUE_FAILED);
            return Err(error);
        }
        let result = self.run_loaded_with_control(
            program,
            observer,
            section,
            tasks,
            execute_callback,
            fanout_callback,
            true,
        );
        let ok = result.is_ok();
        observer.observe(
            &self.execution,
            section,
            if ok {
                detail::LUA_PROLOGUE_SUCCEEDED
            } else {
                detail::LUA_PROLOGUE_FAILED
            },
        );
        result
    }

    /// Binds the model reply for a later epilog in the same environment.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected, the tool
    /// scope remains open, or the reply cannot be installed.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_core::lua::SectionVm;
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::StoreRef;
    ///
    /// let mut vm = SectionVm::new(None, "example-run", &NullObserver, "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &StoreRef::memory(), None)?;
    /// vm.close_tool_scope(&NullObserver, "Example")?;
    /// vm.bind_reply("model answer", &NullObserver, "Example")?;
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn bind_reply(
        &self,
        reply: &str,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<()> {
        observer.observe(&self.execution, section, detail::LUA_REPLY_BINDING_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(&self.execution, section, detail::LUA_REPLY_BINDING_FAILED);
            return Err(error);
        }
        if let Err(error) = self.require_closed_tool_scope("bind a model reply") {
            observer.observe(&self.execution, section, detail::LUA_REPLY_BINDING_FAILED);
            return Err(error);
        }
        let result = self
            .lua
            .globals()
            .raw_set("reply", reply)
            .map_err(|error| Error::Lua(error.to_string()));
        observer.observe(
            &self.execution,
            section,
            if result.is_ok() {
                detail::LUA_REPLY_BINDING_SUCCEEDED
            } else {
                detail::LUA_REPLY_BINDING_FAILED
            },
        );
        result
    }

    /// Executes a compiled epilog in this VM's persistent environment.
    ///
    /// StoreRef-operation reports are delivered in operation order between the
    /// epilog's start and outcome reports. `log(message)` is available only for
    /// this call and reports under `execution` and `section`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected, the tool
    /// scope remains open, execution fails, the shared instruction budget is
    /// exhausted, or the program returns a non-scalar value.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_core::lua::{LuaProgram, SectionVm};
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::StoreRef;
    ///
    /// let epilog = LuaProgram::compile(
    ///     "return reply",
    ///     "epilog",
    ///     1,
    ///     "example-run",
    ///     &NullObserver,
    ///     "Example",
    /// )?;
    /// let mut vm = SectionVm::new(None, "example-run", &NullObserver, "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &StoreRef::memory(), None)?;
    /// vm.close_tool_scope(&NullObserver, "Example")?;
    /// vm.bind_reply("done", &NullObserver, "Example")?;
    /// assert_eq!(
    ///     vm.run_epilog(&epilog, &NullObserver, "Example")?.as_deref(),
    ///     Some("done"),
    /// );
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn run_epilog(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        observer.observe(&self.execution, section, detail::LUA_EPILOG_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(&self.execution, section, detail::LUA_EPILOG_FAILED);
            return Err(error);
        }
        if let Err(error) = self.require_closed_tool_scope("run an epilog") {
            observer.observe(&self.execution, section, detail::LUA_EPILOG_FAILED);
            return Err(error);
        }
        let result = self.run_loaded_with_host(program, observer, section);
        observer.observe(
            &self.execution,
            section,
            if result.is_ok() {
                detail::LUA_EPILOG_SUCCEEDED
            } else {
                detail::LUA_EPILOG_FAILED
            },
        );
        result
    }

    /// Returns the current `var` table as JSON.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected or `var`
    /// cannot be represented as JSON.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_core::lua::SectionVm;
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::StoreRef;
    ///
    /// let mut vm = SectionVm::new(None, "example-run", &NullObserver, "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &StoreRef::memory(), None)?;
    /// assert_eq!(vm.var()?, serde_json::json!({}));
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn var(&self) -> Result<Json> {
        if !self.host_injected {
            return Err(Error::Lua(
                "section VM host values have not been injected".to_owned(),
            ));
        }
        let value: Value = self
            .lua
            .globals()
            .get("var")
            .map_err(|error| Error::Lua(error.to_string()))?;
        self.lua
            .from_value(value)
            .map_err(|error| Error::Lua(error.to_string()))
    }

    /// Sets a string global in the VM, overwriting any existing value.
    ///
    /// Used by fanout to inject `item` after host injection.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the global cannot be set.
    pub(crate) fn set_global_string(&self, name: &str, value: &str) -> Result<()> {
        self.lua
            .globals()
            .raw_set(name, value)
            .map_err(|error| Error::Lua(error.to_string()))
    }

    /// Executes a compiled epilog with `tasks`, `execute`, `jump`, and optional `fanout`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected, the tool
    /// scope is open, or execution fails.
    pub(crate) fn run_epilog_with_control<E, F>(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
        tasks: &[LuaSectionHandle],
        execute_callback: Option<&E>,
        fanout_callback: Option<&F>,
    ) -> Result<LuaBlockResult>
    where
        E: Fn(Value, Option<String>) -> std::result::Result<String, String>,
        F: Fn(String, String) -> std::result::Result<Vec<LuaFanoutResult>, String>,
    {
        observer.observe(&self.execution, section, detail::LUA_EPILOG_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(&self.execution, section, detail::LUA_EPILOG_FAILED);
            return Err(error);
        }
        if let Err(error) = self.require_closed_tool_scope("run an epilog") {
            observer.observe(&self.execution, section, detail::LUA_EPILOG_FAILED);
            return Err(error);
        }
        let result = self.run_loaded_with_control(
            program,
            observer,
            section,
            tasks,
            execute_callback,
            fanout_callback,
            true,
        );
        let ok = result.is_ok();
        observer.observe(
            &self.execution,
            section,
            if ok {
                detail::LUA_EPILOG_SUCCEEDED
            } else {
                detail::LUA_EPILOG_FAILED
            },
        );
        result
    }

    /// Closes and returns this section's effective tool scope.
    ///
    /// Prompt-wide `tools.always` aliases come first, followed by first-seen
    /// `tools.add` aliases from the H2 prologue. Closing is one-way: retained
    /// function references cannot add tools during an epilog.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] for a poisoned declaration runtime, a closure
    /// attempt before host injection, or a second closure attempt.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::lua::{SectionVm, ToolBindings};
    /// use promptforge_core::model::ModelBindings;
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::StoreRef;
    /// let mut vm = SectionVm::new_for_section(
    ///     None,
    ///     &ToolBindings::default(),
    ///     &ModelBindings::default(),
    ///     "example-run",
    ///     &NullObserver,
    ///     "Example",
    /// )?;
    /// vm.inject_host("", &serde_json::json!({}), &StoreRef::memory(), None)?;
    /// let scope = vm.close_tool_scope(&NullObserver, "Example")?;
    /// assert!(scope.bindings().is_empty());
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    /// Closes and returns this section's effective tool scope.
    ///
    /// Also closes model selection recording. Prefer [`Self::close_scopes`] when
    /// the caller needs the section's `models.use` selection.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] for a poisoned declaration runtime, a closure
    /// attempt before host injection, or a second closure attempt.
    #[cfg(test)]
    pub(crate) fn close_tool_scope(
        &self,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<ToolScope> {
        Ok(self.close_scopes(observer, section)?.tools)
    }

    /// Closes tool and model H2 recording for this section.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] for a poisoned declaration runtime, a closure
    /// attempt before host injection, or a second closure attempt.
    pub(crate) fn close_scopes(
        &self,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<ClosedScopes> {
        observer.observe(&self.execution, section, detail::TOOL_SCOPE_CLOSING);
        // LUA-008: validate and compute BOTH scopes before committing either, so
        // a model-close failure cannot leave the tool scope already committed to
        // Closed. Phase 1 validates the tool phase and computes the effective
        // tool scope WITHOUT committing; the model close then validates+resolves;
        // only after both succeed is the (infallible) tool commit performed.
        let tools = self.prepare_tool_scope();
        observer.observe(
            &self.execution,
            section,
            if tools.is_ok() {
                detail::TOOL_SCOPE_CLOSED
            } else {
                detail::TOOL_SCOPE_FAILED
            },
        );
        let tools = tools?;
        let model = close_model_scope(
            &self.bound_models,
            &self.model_runtime,
            &self.execution,
            observer,
            section,
        )?;
        // Both scopes validated (and the model phase committed); the tool commit
        // below only flips an enum and cannot fail.
        self.commit_tool_scope_closed()?;
        Ok(ClosedScopes { tools, model })
    }

    /// Validates the tool phase is open and computes the effective tool scope
    /// WITHOUT committing the phase transition (see [`Self::close_scopes`]).
    fn prepare_tool_scope(&self) -> Result<ToolScope> {
        let bindings = &self.bound_tools;
        let runtime = self
            .tool_runtime
            .lock()
            .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
        if runtime.phase != ToolPhase::H2 {
            return Err(Error::Lua(
                "tool scope can only close once after H2 recording".to_owned(),
            ));
        }
        let aliases = bindings
            .always
            .iter()
            .chain(runtime.added.iter())
            .cloned()
            .collect::<Vec<_>>();
        let effective = aliases
            .iter()
            .map(|alias| binding_for_scope(bindings, &runtime, alias))
            .collect::<Result<Vec<_>>>()?;
        Ok(ToolScope {
            bindings: effective,
        })
    }

    /// Commits the tool scope's H2 -> Closed transition. Infallible apart from a
    /// poisoned lock; only transitions from `H2` so a double call is safe.
    fn commit_tool_scope_closed(&self) -> Result<()> {
        let mut runtime = self
            .tool_runtime
            .lock()
            .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
        if runtime.phase == ToolPhase::H2 {
            runtime.phase = ToolPhase::Closed;
        }
        Ok(())
    }

    /// Installs `tools.calls` as a read-only Lua table backed by the shared
    /// [`ToolCallCounts`]. Each in-scope alias reads its live count; indexing
    /// an unknown key is a hard error that names the bad key and lists the
    /// in-scope set. When the key was declared by `tools.need` but not added
    /// to this section's scope, the diagnostic says so.
    ///
    /// Returns the `ToolCallCounts` handle so the executor's tool loop can
    /// increment it. Reuses counts already seeded by `model:infer` so counters
    /// persist across infer and the prose path.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when installing the `tools.calls` index fails.
    pub(crate) fn install_tool_call_counts(&mut self, scope: &ToolScope) -> Result<ToolCallCounts> {
        let counts = {
            let mut slot = self
                .counts_slot
                .lock()
                .map_err(|_| Error::Lua("tool call counts mutex was poisoned".to_owned()))?;
            if let Some(existing) = slot.as_ref() {
                for binding in scope.bindings() {
                    existing.ensure(binding.alias())?;
                }
                existing.clone()
            } else {
                let created =
                    ToolCallCounts::new(scope.bindings().iter().map(|b| b.alias().to_owned()));
                *slot = Some(created.clone());
                created
            }
        };
        let declared: Vec<String> = self
            .bound_tools
            .bindings()
            .iter()
            .map(|binding| binding.alias().to_owned())
            .collect();
        install_lua_tool_calls(&self.lua, &counts, &declared)?;
        Ok(counts)
    }

    /// Returns frozen tool bindings and the live H2 addition runtime.
    #[must_use]
    pub(crate) fn tool_bag_handles(&self) -> (ToolBindings, Arc<Mutex<ToolRuntime>>) {
        (self.bound_tools.clone(), Arc::clone(&self.tool_runtime))
    }

    /// Returns the shared tool-call counts slot for `model:infer`.
    #[must_use]
    pub(crate) fn counts_slot(&self) -> Arc<Mutex<Option<ToolCallCounts>>> {
        Arc::clone(&self.counts_slot)
    }

    /// Applies the run's Lua resource limits to this VM.
    ///
    /// Sets the heap ceiling (`lua_memory_bytes`) and resets the `log()` event
    /// budget (`lua_log_events`). Called by the executor right after
    /// construction so a VM honors the caller's [`RunLimits`] rather than only
    /// the safe non-env defaults installed in [`SectionVm::new`].
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the underlying VM rejects the memory limit.
    pub(crate) fn apply_lua_limits(&self, memory_bytes: usize, log_events: u32) -> Result<()> {
        self.lua
            .set_memory_limit(memory_bytes)
            .map_err(|error| Error::Lua(error.to_string()))?;
        self.log_budget.store(log_events, Ordering::Relaxed);
        self.log_byte_budget
            .store(default_log_byte_budget(log_events), Ordering::Relaxed);
        Ok(())
    }

    /// Installs the `model:infer` host hook for this VM's Lua state.
    pub(crate) fn set_infer_hook(&self, hook: ModelInferHook) {
        self.lua.set_app_data(hook);
    }

    /// Clears the `model:infer` host hook.
    pub(crate) fn clear_infer_hook(&self) {
        let _ = self.lua.remove_app_data::<ModelInferHook>();
    }

    fn require_closed_tool_scope(&self, operation: &str) -> Result<()> {
        let runtime = self
            .tool_runtime
            .lock()
            .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
        if runtime.phase == ToolPhase::Closed {
            Ok(())
        } else {
            Err(Error::Lua(format!(
                "tool scope must close before the section VM can {operation}"
            )))
        }
    }

    /// Destroys this section VM at an explicit observed lifecycle boundary.
    ///
    /// The observer is borrowed only for this synchronous call and is not
    /// retained by the VM.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_core::lua::SectionVm;
    /// use promptforge_core::observe::NullObserver;
    ///
    /// let vm = SectionVm::new(None, "example-run", &NullObserver, "Example")?;
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn teardown(self, observer: &dyn Observer, section: &str) {
        let execution = self.execution.clone();
        observer.observe(&self.execution, section, detail::LUA_TEARDOWN_STARTED);
        self.clear_infer_hook();
        drop(self);
        observer.observe(&execution, section, detail::LUA_TEARDOWN_SUCCEEDED);
    }

    fn construction_failed(
        self,
        error: Error,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Self> {
        self.teardown(observer, section);
        Err(error)
    }

    fn run_loaded_with_log(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        let returned: MultiValue = self
            .lua
            .scope(|scope| {
                install_log(
                    &self.lua,
                    scope,
                    &self.execution,
                    observer,
                    section,
                    &self.log_budget,
                    &self.log_byte_budget,
                )
                .map_err(|error| mlua::Error::external(error.to_string()))?;
                let result = program
                    .load(&self.lua)
                    .map_err(|error| mlua::Error::external(error.to_string()))?
                    .call(());
                finish_log_phase(&self.lua, result)
            })
            .map_err(|error| program.map_runtime_error(&error))?;
        scalar_return(returned)
    }

    fn run_loaded_without_host(&self, program: &LuaProgram) -> Result<Option<String>> {
        let returned: MultiValue = program
            .load(&self.lua)?
            .call(())
            .map_err(|error| program.map_runtime_error(&error))?;
        scalar_return(returned)
    }

    fn run_loaded_with_host(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        let store = self.store.as_ref().ok_or_else(|| {
            Error::Lua("section VM host values have not been injected".to_owned())
        })?;
        let returned: MultiValue = self
            .lua
            .scope(|scope| {
                install_log(
                    &self.lua,
                    scope,
                    &self.execution,
                    observer,
                    section,
                    &self.log_budget,
                    &self.log_byte_budget,
                )
                .map_err(|error| mlua::Error::external(error.to_string()))?;
                install_store_table(
                    &self.lua,
                    scope,
                    &self.lua.globals(),
                    store,
                    &self.execution,
                    observer,
                    section,
                )
                .map_err(|error| mlua::Error::external(error.to_string()))?;
                let result = program
                    .load(&self.lua)
                    .map_err(|error| mlua::Error::external(error.to_string()))?
                    .call(());
                finish_log_phase(&self.lua, result)
            })
            .map_err(|error| program.map_runtime_error(&error))?;
        scalar_return(returned)
    }

    fn take_jump(&self) -> Option<String> {
        self.jump_slot.lock().ok().and_then(|mut slot| slot.take())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "control-flow host fns are installed together for one Lua phase"
    )]
    fn run_loaded_with_control<E, F>(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
        tasks: &[LuaSectionHandle],
        execute_callback: Option<&E>,
        fanout_callback: Option<&F>,
        jump_enabled: bool,
    ) -> Result<LuaBlockResult>
    where
        E: Fn(Value, Option<String>) -> std::result::Result<String, String>,
        F: Fn(String, String) -> std::result::Result<Vec<LuaFanoutResult>, String>,
    {
        let store = self.store.as_ref().ok_or_else(|| {
            Error::Lua("section VM host values have not been injected".to_owned())
        })?;
        if let Ok(mut slot) = self.jump_slot.lock() {
            *slot = None;
        }
        let jump_slot = Arc::clone(&self.jump_slot);
        let result = self.lua.scope(|scope| {
            install_log(
                &self.lua,
                scope,
                &self.execution,
                observer,
                section,
                &self.log_budget,
                &self.log_byte_budget,
            )
            .map_err(|error| mlua::Error::external(error.to_string()))?;
            install_store_table(
                &self.lua,
                scope,
                &self.lua.globals(),
                store,
                &self.execution,
                observer,
                section,
            )
            .map_err(|error| mlua::Error::external(error.to_string()))?;
            install_tasks_table(&self.lua, tasks)
                .map_err(|error| mlua::Error::external(error.to_string()))?;
            if let Some(execute_callback) = execute_callback {
                let execute_fn = scope
                    .create_function(|_, (target, input): (Value, Option<String>)| {
                        execute_callback(target, input).map_err(mlua::Error::external)
                    })
                    .map_err(|error| mlua::Error::external(error.to_string()))?;
                self.lua
                    .globals()
                    .raw_set("execute", execute_fn)
                    .map_err(|error| mlua::Error::external(error.to_string()))?;
            }
            if jump_enabled {
                let jump_fn = scope
                    .create_function(move |_, target: Value| -> mlua::Result<()> {
                        let heading = resolve_section_target(target)?;
                        let mut slot = jump_slot
                            .lock()
                            .map_err(|_| mlua::Error::external("jump slot poisoned"))?;
                        *slot = Some(heading);
                        Err(mlua::Error::external("jump transfer"))
                    })
                    .map_err(|error| mlua::Error::external(error.to_string()))?;
                self.lua
                    .globals()
                    .raw_set("jump", jump_fn)
                    .map_err(|error| mlua::Error::external(error.to_string()))?;
            }
            if let Some(fanout_callback) = fanout_callback {
                let fanout_fn = scope
                    .create_function(|lua, (worker, list): (String, String)| {
                        let replies =
                            fanout_callback(worker, list).map_err(mlua::Error::external)?;
                        let table = lua.create_table_with_capacity(replies.len(), 0)?;
                        for (i, reply) in replies.into_iter().enumerate() {
                            table.raw_set(i + 1, reply)?;
                        }
                        Ok(table)
                    })
                    .map_err(|error| mlua::Error::external(error.to_string()))?;
                self.lua
                    .globals()
                    .raw_set("fanout", fanout_fn)
                    .map_err(|error| mlua::Error::external(error.to_string()))?;
            }
            let result = program
                .load(&self.lua)
                .map_err(|error| mlua::Error::external(error.to_string()))?
                .call(());
            finish_log_phase(&self.lua, result)
        });
        // Control-global cleanup runs on EVERY exit (jump, success, or ordinary
        // execution error), so a failing block never leaks live `jump`/`execute`/
        // `fanout`/`tasks` globals into a later phase (LUA-007). Cleanup failures
        // are combined with the execution outcome rather than discarded.
        let jump = self.take_jump();
        let cleanup = self.clear_control_globals();
        if let Some(heading) = jump {
            cleanup?;
            return Ok(LuaBlockResult::Jump(heading));
        }
        let returned = result.map_err(|error| program.map_runtime_error(&error));
        match (returned, cleanup) {
            // Execution error is the primary cause; it takes precedence.
            (Err(execution), _) => Err(execution),
            // Execution succeeded but cleanup failed: surface the cleanup error.
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Ok(values), Ok(())) => Ok(LuaBlockResult::Returned(scalar_return(values)?)),
        }
    }

    /// Clears the phase's control-flow globals, returning the first failure.
    ///
    /// Always attempts to clear every global even if an earlier clear fails, so
    /// no live control function is left installed for the next phase.
    fn clear_control_globals(&self) -> Result<()> {
        let globals = self.lua.globals();
        let mut first_error: Option<Error> = None;
        for name in ["jump", "execute", "fanout", "tasks"] {
            if let Err(error) = globals.raw_set(name, Value::Nil)
                && first_error.is_none()
            {
                first_error = Some(Error::Lua(error.to_string()));
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    #[cfg(test)]
    fn run_source(
        &self,
        source: &str,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        let store = self.store.as_ref().ok_or_else(|| {
            Error::Lua("section VM host values have not been injected".to_owned())
        })?;
        let returned: MultiValue = self
            .lua
            .scope(|scope| {
                install_log(
                    &self.lua,
                    scope,
                    &self.execution,
                    observer,
                    section,
                    &self.log_budget,
                    &self.log_byte_budget,
                )
                .map_err(|error| mlua::Error::external(error.to_string()))?;
                install_store_table(
                    &self.lua,
                    scope,
                    &self.lua.globals(),
                    store,
                    &self.execution,
                    observer,
                    section,
                )
                .map_err(|error| mlua::Error::external(error.to_string()))?;
                let result = self.lua.load(source).eval();
                finish_log_phase(&self.lua, result)
            })
            .map_err(|error| Error::Lua(error.to_string()))?;
        scalar_return(returned)
    }
}

/// The result of running a section's Lua block.
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct LuaOutcome {
    /// The chunk's top-level return value, if it returned one (the finish case).
    pub(crate) returned: Option<String>,
    /// The `var` table after the block ran, as JSON, for prose substitution.
    pub(crate) var: Json,
}

/// Run a section's Lua chunk with `args` and `sys` exposed, a writable `var`
/// table available, and a `store` table backed by `store`, returning the
/// chunk's return value and the final `var`. Harness-mediated store operations
/// report safe outcomes to `observer` under `execution` and `section`.
/// `log(message)` reports constrained author checkpoints through the same
/// observer for this call only; direct `print` is unavailable.
///
/// `store` is the run-scoped virtual-file handle; every section in a run is
/// given the same handle, so files a section writes persist for later sections
/// even though each section starts a fresh context. The exposed `store` table
/// is always present (a host capability, not a scoped tool).
///
/// The `tools` table is the same validating one every section VM installs,
/// with no frozen bindings: a chunk that calls `tools.add(...)` fails loudly
/// because no alias was declared by `tools.need`.
///
/// # Errors
/// Returns [`Error::Lua`] if the sandbox cannot be built, `sys`/`var`/`store`
/// cannot be bridged, the chunk fails to run (including hitting the instruction
/// budget or a failing `store` op, which raises a Lua error), or it returns a
/// value that cannot be rendered as a result string.
#[cfg(test)]
pub(crate) fn run_chunk(
    source: &str,
    args: &str,
    sys: &Json,
    store: &StoreRef,
    execution: &str,
    observer: &dyn Observer,
    section: &str,
) -> Result<LuaOutcome> {
    let mut vm = SectionVm::new(None, execution, observer, section)?;
    vm.inject_host(args, sys, store, None)?;
    let returned = vm.run_source(source, observer, section)?;
    let var = vm.var()?;

    Ok(LuaOutcome { returned, var })
}

/// Installs `tools.calls` on the existing `tools` global as a read-only table.
///
/// Reading a known alias returns its current count from `counts`. Indexing an
/// unknown key raises a hard Lua error naming the bad key and listing the VM's
/// in-scope aliases. `declared` is the prompt-wide `tools.need` set used to
/// distinguish pure unknowns from declared-but-unscoped aliases.
/// Snapshot-reads always + H2 additions without closing the tool phase.
pub(crate) fn snapshot_tool_scope(
    bindings: &ToolBindings,
    runtime: &Mutex<ToolRuntime>,
) -> Result<ToolScope> {
    let runtime = runtime
        .lock()
        .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
    let aliases = bindings
        .always
        .iter()
        .chain(runtime.added.iter())
        .cloned()
        .collect::<Vec<_>>();
    let effective = aliases
        .iter()
        .map(|alias| binding_for_scope(bindings, &runtime, alias))
        .collect::<Result<Vec<_>>>()?;
    Ok(ToolScope {
        bindings: effective,
    })
}

/// Clones a frozen binding and applies any author model-description override.
fn binding_for_scope(
    bindings: &ToolBindings,
    runtime: &ToolRuntime,
    alias: &str,
) -> Result<ToolBinding> {
    let mut binding = bindings
        .binding(alias)
        .cloned()
        .ok_or_else(|| Error::Lua(format!("tool alias {alias:?} has no frozen binding")))?;
    if let Some(description) = runtime.description_overrides.get(alias) {
        binding.model_description = Some(description.clone());
    }
    Ok(binding)
}

pub(crate) fn install_lua_tool_calls(
    lua: &Lua,
    counts: &ToolCallCounts,
    declared: &[String],
) -> Result<()> {
    let globals = lua.globals();
    let tools: mlua::Table = globals
        .raw_get("tools")
        .map_err(|error| Error::Lua(error.to_string()))?;

    let calls_inner = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;
    let meta = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;

    let counts_for_index = counts.clone();
    let declared: Vec<String> = declared.to_vec();
    let index = lua
        .create_function(move |_, (_table, key): (mlua::Table, String)| {
            let value = counts_for_index
                .get(&key)
                .map_err(|e| mlua::Error::external(e.to_string()))?;
            if let Some(count) = value {
                Ok(count)
            } else {
                let in_scope = counts_for_index
                    .aliases()
                    .map_err(|e| mlua::Error::external(e.to_string()))?;
                let declared_unscoped = declared.iter().any(|alias| alias == &key);
                Err(mlua::Error::external(format!(
                    "tools.calls: {key:?} is not in this section's tool scope; \
                     in-scope aliases: {in_scope:?}{}",
                    if declared_unscoped {
                        " (alias was declared by tools.need but not added to this section's scope)"
                    } else if in_scope.is_empty() {
                        ""
                    } else {
                        " - check for typos or add it via tools.add"
                    }
                )))
            }
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    meta.set("__index", index)
        .map_err(|error| Error::Lua(error.to_string()))?;

    let newindex_err = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external("tools.calls is read-only"))
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    meta.set("__newindex", newindex_err)
        .map_err(|error| Error::Lua(error.to_string()))?;

    calls_inner.set_metatable(Some(meta));

    tools
        .set("calls", calls_inner)
        .map_err(|error| Error::Lua(error.to_string()))?;
    Ok(())
}

/// One flattened `tools.add` entry: alias plus optional model-description override.
struct ToolsAddEntry {
    alias: String,
    description_override: Option<String>,
}

/// Collects add entries from one `tools.add` argument.
///
/// Accepts a UTF-8 string, a [`LuaToolHandle`], or a sequence table of either.
/// A Tool handle contributes a description override only when the author
/// assigned `.description` on that object.
fn push_tools_add_entry(entries: &mut Vec<ToolsAddEntry>, value: Value) -> mlua::Result<()> {
    match value {
        Value::String(s) => {
            entries.push(ToolsAddEntry {
                alias: s.to_string_lossy(),
                description_override: None,
            });
            Ok(())
        }
        Value::UserData(ud) => {
            let handle = ud.borrow::<LuaToolHandle>()?;
            entries.push(ToolsAddEntry {
                alias: handle.name().to_owned(),
                description_override: handle.model_description_override().map(str::to_owned),
            });
            Ok(())
        }
        Value::Table(table) => {
            for item in table.sequence_values::<Value>() {
                match item? {
                    Value::String(s) => entries.push(ToolsAddEntry {
                        alias: s.to_string_lossy(),
                        description_override: None,
                    }),
                    Value::UserData(ud) => {
                        let handle = ud.borrow::<LuaToolHandle>()?;
                        entries.push(ToolsAddEntry {
                            alias: handle.name().to_owned(),
                            description_override: handle
                                .model_description_override()
                                .map(str::to_owned),
                        });
                    }
                    _ => {
                        return Err(mlua::Error::external(
                            "tools.add array elements must be strings or Tool objects",
                        ));
                    }
                }
            }
            Ok(())
        }
        _ => Err(mlua::Error::external(
            "tools.add expects strings, Tool objects, or arrays of either",
        )),
    }
}

/// Flattens a `tools.add` variadic into alias/override entries for scope.
fn collect_tools_add_entries(args: Variadic<Value>) -> mlua::Result<Vec<ToolsAddEntry>> {
    let mut entries = Vec::new();
    for value in args {
        push_tools_add_entry(&mut entries, value)?;
    }
    Ok(entries)
}

fn install_h2_tools(
    lua: &Lua,
    globals: &mlua::Table,
    bindings: &ToolBindings,
    runtime: &Arc<Mutex<ToolRuntime>>,
) -> Result<()> {
    {
        let state = runtime
            .lock()
            .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
        if state.phase != ToolPhase::H2 {
            return Err(Error::Lua(
                "tool scope is not open for H2 recording".to_owned(),
            ));
        }
    }

    let tools = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;
    for name in ["need", "always"] {
        let operation = name;
        let forbidden = lua
            .create_function(move |_, _: MultiValue| -> mlua::Result<()> {
                Err(mlua::Error::external(format!(
                    "tools.{operation} is only available during live H1 execution"
                )))
            })
            .map_err(|error| Error::Lua(error.to_string()))?;
        tools
            .set(name, forbidden)
            .map_err(|error| Error::Lua(error.to_string()))?;
    }

    let frozen = bindings.clone();
    let state = Arc::clone(runtime);
    let add = lua
        .create_function(move |_, args: Variadic<Value>| {
            let entries = collect_tools_add_entries(args)?;
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("tool declaration runtime was poisoned"))?;
            if state.phase != ToolPhase::H2 {
                return Err(mlua::Error::external(
                    "tools.add is only available before the H2 tool scope closes",
                ));
            }
            for entry in &entries {
                validate_alias(&entry.alias)
                    .map_err(|error| mlua::Error::external(error.to_string()))?;
                if frozen.binding(&entry.alias).is_none() {
                    return Err(mlua::Error::external(format!(
                        "tools.add alias {:?} was not declared by tools.need",
                        entry.alias
                    )));
                }
            }
            let mut changed = false;
            for entry in entries {
                if let Some(description) = entry.description_override {
                    let override_changed = match state.description_overrides.get(&entry.alias) {
                        Some(existing) => existing != &description,
                        None => true,
                    };
                    if override_changed {
                        state
                            .description_overrides
                            .insert(entry.alias.clone(), description);
                        changed = true;
                    }
                }
                if frozen
                    .always
                    .iter()
                    .any(|existing| existing == &entry.alias)
                {
                    continue;
                }
                if !state.added.iter().any(|existing| existing == &entry.alias) {
                    state.added.push(entry.alias);
                    changed = true;
                }
            }
            if changed {
                state.generation = state.generation.saturating_add(1);
            }
            Ok(())
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    tools
        .set("add", add)
        .map_err(|error| Error::Lua(error.to_string()))?;
    globals
        .raw_set("tools", tools)
        .map_err(|error| Error::Lua(error.to_string()))
}

/// Installs the phase-local author diagnostic callback.
///
/// The callback borrows its observer through [`Scope`], so neither the callback
/// nor any Lua reference copied from it can retain that observer after the
/// current H1 or H2 phase returns.
fn install_tasks_table(lua: &Lua, tasks: &[LuaSectionHandle]) -> Result<()> {
    let table = lua
        .create_table_with_capacity(0, tasks.len())
        .map_err(|error| Error::Lua(error.to_string()))?;
    for handle in tasks {
        let userdata = lua
            .create_userdata(handle.clone())
            .map_err(|error| Error::Lua(error.to_string()))?;
        table
            .raw_set(handle.heading(), userdata)
            .map_err(|error| Error::Lua(error.to_string()))?;
    }
    lua.globals()
        .raw_set("tasks", table)
        .map_err(|error| Error::Lua(error.to_string()))
}

mod hardening;
pub(crate) use hardening::*;
mod sys;
pub(crate) use sys::*;
mod host;
pub(crate) use host::*;
