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

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use mlua::{
    Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, MetaMethod, MultiValue, Scope, StdLib,
    UserData, UserDataFields, UserDataMethods, Value, Variadic, VmState,
};
use serde_json::Value as Json;
use serde_json::json;

pub(crate) use crate::lua_models::{LuaModelHandle, ModelInferHook};
use crate::lua_models::{
    ModelBindingState, ModelRuntime, close_model_scope, install_h2_models, install_live_models,
};
use crate::model::{ModelBinding, ModelBindings, ModelResolver};
use crate::observe::{Observation, Observer, detail};
use crate::resolve::RuntimeResolution;
use crate::store::StoreRef;
use crate::tools::{ToolId, ToolRegistry};
use crate::{Error, Result};

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaProgram {
    source: String,
    bytecode: Vec<u8>,
    /// Parser location string used as the Lua chunk name (for example
    /// `section \`Web Search\` epilog`).
    location: String,
    /// 1-based line number in the prompt source where this Lua region begins.
    ///
    /// Used together with chunk-relative line numbers from Lua runtime errors
    /// to produce an absolute prompt-source line: `source_line + chunk_line - 1`.
    source_line: u32,
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
    /// ```ignore
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
        source_line: u32,
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
                    source_line,
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
    pub fn source_line(&self) -> u32 {
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
        let raw = error.to_string();
        let mapped = map_chunk_line_to_absolute(&raw, self.source_line, self.location());
        Error::Lua(mapped)
    }

    /// Chunk name recorded at compile time (parser location string).
    #[must_use]
    pub fn location(&self) -> &str {
        &self.location
    }
}

/// Rewrites chunk-relative line numbers for one named chunk to absolute
/// prompt-source lines.
///
/// Only `[string "{location}"]:N:` occurrences are rewritten, so a parent
/// prologue that surfaces a fanout child's already-mapped error does not
/// corrupt the child's absolute line. When `source_line` is 0 or the pattern
/// is absent, the message passes through unchanged except for a leading
/// `{location}:` tag when an absolute line can still be inferred.
fn map_chunk_line_to_absolute(message: &str, source_line: u32, location: &str) -> String {
    if source_line == 0 || location.is_empty() {
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
            let absolute = source_line + chunk_line - 1;
            if first_absolute.is_none() {
                first_absolute = Some(absolute);
            }
            result.push_str(&absolute.to_string());
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
/// ```ignore
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
    /// ```ignore
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
    /// ```ignore
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
    pub(crate) fn current_sys(&self, fallback: &Json) -> Json {
        self.sys_live
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_else(|| fallback.clone())
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
    /// ```ignore
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
    /// ```ignore
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
    /// ```ignore
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
    /// ```ignore
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
        let tools = self.close_tool_scope_inner();
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
        Ok(ClosedScopes { tools, model })
    }

    fn close_tool_scope_inner(&self) -> Result<ToolScope> {
        let bindings = &self.bound_tools;
        let mut runtime = self
            .tool_runtime
            .lock()
            .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
        if runtime.phase != ToolPhase::H2 {
            return Err(Error::Lua(
                "tool scope can only close once after H2 recording".to_owned(),
            ));
        }
        runtime.phase = ToolPhase::Closed;
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
    /// ```ignore
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
        if let Some(heading) = self.take_jump() {
            let _ = self.lua.globals().raw_set("jump", Value::Nil);
            let _ = self.lua.globals().raw_set("execute", Value::Nil);
            let _ = self.lua.globals().raw_set("fanout", Value::Nil);
            let _ = self.lua.globals().raw_set("tasks", Value::Nil);
            return Ok(LuaBlockResult::Jump(heading));
        }
        let returned: MultiValue = result.map_err(|error| program.map_runtime_error(&error))?;
        let _ = self.lua.globals().raw_set("jump", Value::Nil);
        let _ = self.lua.globals().raw_set("execute", Value::Nil);
        let _ = self.lua.globals().raw_set("fanout", Value::Nil);
        let _ = self.lua.globals().raw_set("tasks", Value::Nil);
        Ok(LuaBlockResult::Returned(scalar_return(returned)?))
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

fn install_log<'scope, 'env: 'scope>(
    lua: &Lua,
    scope: &'scope Scope<'scope, 'env>,
    execution: &'env str,
    observer: &'env dyn Observer,
    section: &'env str,
    log_budget: &'env AtomicU32,
) -> Result<()> {
    let log = scope
        .create_function(move |_, arguments: MultiValue| {
            if arguments.len() != 1 {
                return Err(mlua::Error::external("log expects exactly one argument"));
            }
            // Spend one unit of the per-VM log budget before doing any work; an
            // exhausted budget refuses further checkpoints (lua 002).
            if log_budget
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
                .is_err()
            {
                return Err(mlua::Error::external("lua log event budget exceeded"));
            }
            let Some(Value::String(message)) = arguments.into_iter().next() else {
                return Err(mlua::Error::external("log message must be a UTF-8 string"));
            };
            let message = message
                .to_str()
                .map_err(|_| mlua::Error::external("log message must be a UTF-8 string"))?;
            if message.chars().count() > LUA_LOG_CHARACTER_LIMIT {
                return Err(mlua::Error::external(
                    "log message must be at most 256 characters",
                ));
            }
            if message.chars().any(is_log_line_break_or_control) {
                return Err(mlua::Error::external(
                    "log message must not contain newline or control characters",
                ));
            }
            observer.observe(execution, section, Observation::Lua(message.to_owned()));
            Ok(())
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    lua.globals()
        .raw_set("log", log)
        .map_err(|error| Error::Lua(error.to_string()))
}

fn is_log_line_break_or_control(character: char) -> bool {
    character.is_control() || matches!(character, '\u{2028}' | '\u{2029}')
}

/// Clears the phase's global callback before its scoped Rust closure expires.
///
/// Lua code may have copied the function elsewhere, but [`Scope`] invalidates
/// every such reference when the phase returns.
fn finish_log_phase<T>(lua: &Lua, result: mlua::Result<T>) -> mlua::Result<T> {
    let cleanup = lua.globals().raw_set("log", Value::Nil);
    match (result, cleanup) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
    }
}

/// Expose an always-on `store` table whose six methods (`write`, `append`,
/// `read`, `str_replace`, `delete`, `glob`) are backed by the run-scoped
/// [`StoreRef`] handle. The functions borrow the observer within an [`mlua::Scope`]
/// so each operation reports immediately after its result is known.
///
/// The table is a deterministic host capability, present regardless of tool
/// scoping. The mutating ops (`write`/`append`/`str_replace`/`delete`) return
/// nil; `read` returns the file's numbered-line string; `glob` returns an
/// array table of matching paths. A [`StoreError`] from any op is mapped into
/// an `mlua` error via [`mlua::Error::external`], so it aborts the chunk and
/// surfaces from [`run_chunk`] as [`Error::Lua`].
///
/// The `StoreRef` handle locks a mutex internally per call and is synchronous, so
/// nothing is held across an await.
///
/// [`StoreError`]: crate::store::StoreError
///
/// # Errors
/// Returns [`Error::Lua`] if the `store` table or any of its functions cannot
/// be created or installed into the sandbox globals.
fn observe_store_result(
    execution: &str,
    observer: &dyn Observer,
    section: &str,
    succeeded: bool,
    success: Observation,
    failure: Observation,
) {
    observer.observe(
        execution,
        section,
        if succeeded { success } else { failure },
    );
}

#[expect(
    clippy::too_many_lines,
    reason = "one table installs all six store operations beside their matching observation outcomes"
)]
fn install_store_table<'scope, 'env: 'scope>(
    lua: &Lua,
    scope: &'scope Scope<'scope, 'env>,
    globals: &mlua::Table,
    store: &StoreRef,
    execution: &'env str,
    observer: &'env dyn Observer,
    section: &'env str,
) -> Result<()> {
    let table = lua.create_table().map_err(|e| Error::Lua(e.to_string()))?;

    let handle = store.clone();
    let write = scope
        .create_function(move |_, (path, contents): (String, String)| {
            let result = handle.write(&path, &contents);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_WRITE_SUCCEEDED,
                detail::STORE_WRITE_FAILED,
            );
            result.map_err(mlua::Error::external)?;
            Ok(())
        })
        .map_err(|e| Error::Lua(e.to_string()))?;
    table
        .set("write", write)
        .map_err(|e| Error::Lua(e.to_string()))?;

    let handle = store.clone();
    let append = scope
        .create_function(move |_, (path, contents): (String, String)| {
            let result = handle.append(&path, &contents);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_APPEND_SUCCEEDED,
                detail::STORE_APPEND_FAILED,
            );
            result.map_err(mlua::Error::external)?;
            Ok(())
        })
        .map_err(|e| Error::Lua(e.to_string()))?;
    table
        .set("append", append)
        .map_err(|e| Error::Lua(e.to_string()))?;

    let handle = store.clone();
    let read_lines = scope
        .create_function(move |_, path: String| {
            let result = handle.read_lines(&path);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_READ_LINES_SUCCEEDED,
                detail::STORE_READ_LINES_FAILED,
            );
            result.map_err(mlua::Error::external)
        })
        .map_err(|e| Error::Lua(e.to_string()))?;
    table
        .set("read_lines", read_lines)
        .map_err(|e| Error::Lua(e.to_string()))?;

    let handle = store.clone();
    let read = scope
        .create_function(move |_, path: String| {
            let result = handle.read(&path);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_READ_SUCCEEDED,
                detail::STORE_READ_FAILED,
            );
            result.map_err(mlua::Error::external)
        })
        .map_err(|e| Error::Lua(e.to_string()))?;
    table
        .set("read", read)
        .map_err(|e| Error::Lua(e.to_string()))?;

    let handle = store.clone();
    let inject = scope
        .create_function(move |_, path: String| {
            let result = handle.inject(&path);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_INJECT_SUCCEEDED,
                detail::STORE_INJECT_FAILED,
            );
            result.map_err(mlua::Error::external)
        })
        .map_err(|e| Error::Lua(e.to_string()))?;
    table
        .set("inject", inject)
        .map_err(|e| Error::Lua(e.to_string()))?;

    let handle = store.clone();
    let str_replace = scope
        .create_function(move |_, (path, old, new): (String, String, String)| {
            let result = handle.str_replace(&path, &old, &new);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_REPLACE_SUCCEEDED,
                detail::STORE_REPLACE_FAILED,
            );
            result.map_err(mlua::Error::external)?;
            Ok(())
        })
        .map_err(|e| Error::Lua(e.to_string()))?;
    table
        .set("str_replace", str_replace)
        .map_err(|e| Error::Lua(e.to_string()))?;

    let handle = store.clone();
    let delete = scope
        .create_function(move |_, path: String| {
            let result = handle.delete(&path);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_DELETE_SUCCEEDED,
                detail::STORE_DELETE_FAILED,
            );
            result.map_err(mlua::Error::external)?;
            Ok(())
        })
        .map_err(|e| Error::Lua(e.to_string()))?;
    table
        .set("delete", delete)
        .map_err(|e| Error::Lua(e.to_string()))?;

    let handle = store.clone();
    let glob = scope
        .create_function(move |lua, pattern: String| {
            let result = handle.glob(&pattern);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_GLOB_SUCCEEDED,
                detail::STORE_GLOB_FAILED,
            );
            let paths = result.map_err(mlua::Error::external)?;
            lua.create_sequence_from(paths)
        })
        .map_err(|e| Error::Lua(e.to_string()))?;
    table
        .set("glob", glob)
        .map_err(|e| Error::Lua(e.to_string()))?;

    let handle = store.clone();
    let exists = scope
        .create_function(move |_, path: String| handle.exists(&path).map_err(mlua::Error::external))
        .map_err(|e| Error::Lua(e.to_string()))?;
    table
        .set("exists", exists)
        .map_err(|e| Error::Lua(e.to_string()))?;

    globals
        .raw_set("store", table)
        .map_err(|e| Error::Lua(e.to_string()))?;
    Ok(())
}

/// Returns a copy of `sys` with the bound catalog model id under `"model"`.
pub(crate) fn enrich_sys_model(sys: &Json, binding: &ModelBinding) -> Json {
    match sys {
        Json::Object(map) => {
            let mut out = map.clone();
            out.insert(
                "model".to_owned(),
                Json::String(binding.id().name().to_owned()),
            );
            Json::Object(out)
        }
        other => other.clone(),
    }
}

/// Returns a copy of `sys` with `reply_finish_reason` set from the last inference.
pub(crate) fn enrich_sys_reply_finish_reason(sys: &Json, reason: Option<&str>) -> Json {
    match sys {
        Json::Object(map) => {
            let mut out = map.clone();
            out.insert(
                "reply_finish_reason".to_owned(),
                match reason {
                    Some(value) => Json::String(value.to_owned()),
                    None => Json::Null,
                },
            );
            Json::Object(out)
        }
        other => other.clone(),
    }
}

/// Builds a sealed Lua `sys` table from runtime metadata.
///
/// The proxy is empty; reads go through `__index` against the JSON object and
/// raise when the field is absent. Present `null` values surface as Lua nil.
/// `__newindex` rejects every write. `__metatable` is set so author code cannot
/// replace the seal.
pub(crate) fn seal_sys(lua: &Lua, sys: &Json) -> Result<mlua::Table> {
    let data = match sys {
        Json::Object(map) => map.clone(),
        other => {
            return Err(Error::Lua(format!("sys must be a table, got {other}")));
        }
    };

    let proxy = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;
    let metatable = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;

    let index = lua
        .create_function(move |lua, (_table, key): (Value, Value)| {
            let Value::String(name) = key else {
                return Err(mlua::Error::runtime(
                    "sys fields must be accessed by string key".to_owned(),
                ));
            };
            let field = name.to_string_lossy();
            match data.get(field.as_str()) {
                None => Err(mlua::Error::runtime(format!("unknown sys field '{field}'"))),
                Some(Json::Null) => Ok(Value::Nil),
                Some(value) => lua.to_value(value),
            }
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    metatable
        .set("__index", index)
        .map_err(|error| Error::Lua(error.to_string()))?;

    let newindex = lua
        .create_function(
            move |_lua, (_table, key, _value): (Value, Value, Value)| -> mlua::Result<()> {
                let field = match key {
                    Value::String(name) => name.to_string_lossy(),
                    other => format!("{other:?}"),
                };
                Err(mlua::Error::runtime(format!(
                    "sys is read-only; cannot set '{field}'"
                )))
            },
        )
        .map_err(|error| Error::Lua(error.to_string()))?;
    metatable
        .set("__newindex", newindex)
        .map_err(|error| Error::Lua(error.to_string()))?;
    metatable
        .set("__metatable", "sys is sealed")
        .map_err(|error| Error::Lua(error.to_string()))?;

    proxy.set_metatable(Some(metatable));
    Ok(proxy)
}

/// Remove code-loading, direct output, and reflection globals the base library
/// provides. The `io`, `os`, `package`, `coroutine`, and `debug` libraries are
/// never loaded.
///
/// Also wraps `table.concat` so userdata with `__tostring` (fanout result
/// objects) coerce like `tostring`, keeping existing `table.concat(results)`
/// callers working after fanout returns structured objects. Tables and
/// booleans still error as stock Lua would.
fn harden(lua: &Lua) -> Result<()> {
    let globals = lua.globals();
    for name in [
        "load",
        "loadstring",
        "dofile",
        "loadfile",
        "collectgarbage",
        "require",
        "getfenv",
        "setfenv",
        "rawget",
        "rawset",
        "rawequal",
        "rawlen",
        "print",
        "warn",
    ] {
        globals
            .set(name, Value::Nil)
            .map_err(|e| Error::Lua(e.to_string()))?;
    }
    lua.load(
        r#"
local orig = table.concat
function table.concat(list, sep, i, j)
  i = i or 1
  j = j or #list
  local parts = {}
  for k = i, j do
    local v = list[k]
    local ty = type(v)
    if ty == "string" or ty == "number" then
      parts[#parts + 1] = v
    elseif ty == "userdata" then
      -- Fanout result objects (and any other userdata with __tostring).
      -- mlua metatables are not readable via getmetatable, so type-gate here.
      parts[#parts + 1] = tostring(v)
    elseif v == nil then
      error("invalid value (nil) at index " .. k .. " in table for 'concat'")
    else
      error("invalid value (" .. ty .. ") at index " .. k .. " in table for 'concat'")
    end
  end
  return orig(parts, sep)
end
"#,
    )
    .exec()
    .map_err(|e| Error::Lua(e.to_string()))?;
    Ok(())
}

/// Install an instruction-count hook that aborts a runaway block.
fn install_instruction_budget(lua: &Lua) {
    let fired = Arc::new(AtomicU64::new(0));
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
        move |_lua, _debug| {
            if fired.fetch_add(1, Ordering::Relaxed) >= HOOK_BUDGET {
                return Err(mlua::Error::RuntimeError(
                    "lua instruction budget exceeded".to_string(),
                ));
            }
            Ok(VmState::Continue)
        },
    );
}

/// Render a returned Lua scalar as the section's result string. Tables and other
/// non-scalar returns are deferred to a later commit.
fn value_to_string(value: &Value) -> Result<String> {
    match value {
        Value::String(s) => Ok(s.to_string_lossy()),
        Value::Integer(i) => Ok(i.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Boolean(b) => Ok(b.to_string()),
        other => Err(Error::Lua(format!(
            "cannot return a {} as a result",
            other.type_name()
        ))),
    }
}

fn scalar_return(returned: MultiValue) -> Result<Option<String>> {
    match returned.into_iter().next() {
        None | Some(Value::Nil) => Ok(None),
        Some(value) => value_to_string(&value).map(Some),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::observe::{NullObserver, Observation};
    use crate::store::{Store, StoreError};
    use crate::tools::{Tool, ToolError, ToolOutput};
    use serde_json::json;

    const EXECUTION: &str = "lua-test";

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, String, Observation)>>);

    impl Observer for Recorder {
        fn observe(&self, execution: &str, section: &str, event: Observation) {
            self.0
                .lock()
                .expect("the recorder mutex must not be poisoned")
                .push((execution.to_owned(), section.to_owned(), event));
        }
    }

    impl Recorder {
        fn records(&self) -> Vec<(String, String, Observation)> {
            self.0
                .lock()
                .expect("the recorder mutex must not be poisoned")
                .clone()
        }

        fn observations(&self) -> Vec<(String, Observation)> {
            self.0
                .lock()
                .expect("the recorder mutex must not be poisoned")
                .iter()
                .map(|(_, section, detail)| (section.clone(), detail.clone()))
                .collect()
        }
    }

    #[derive(Debug)]
    struct FailingStore;

    impl FailingStore {
        fn error(path: &str) -> StoreError {
            StoreError::NotFound {
                path: path.to_owned(),
            }
        }
    }

    impl Store for FailingStore {
        fn write(&mut self, path: &str, _contents: &str) -> std::result::Result<(), StoreError> {
            Err(Self::error(path))
        }

        fn append(&mut self, path: &str, _contents: &str) -> std::result::Result<(), StoreError> {
            Err(Self::error(path))
        }

        fn read_lines(&self, path: &str) -> std::result::Result<String, StoreError> {
            Err(Self::error(path))
        }

        fn read(&self, path: &str) -> std::result::Result<String, StoreError> {
            Err(Self::error(path))
        }

        fn str_replace(
            &mut self,
            path: &str,
            _old: &str,
            _new: &str,
        ) -> std::result::Result<(), StoreError> {
            Err(Self::error(path))
        }

        fn delete(&mut self, path: &str) -> std::result::Result<(), StoreError> {
            Err(Self::error(path))
        }

        fn glob(&self, pattern: &str) -> std::result::Result<Vec<String>, StoreError> {
            Err(Self::error(pattern))
        }

        fn exists(&self, path: &str) -> std::result::Result<bool, StoreError> {
            Err(Self::error(path))
        }
    }

    struct BoundaryRecorder {
        store: StoreRef,
        snapshots: Mutex<Vec<Vec<String>>>,
    }

    impl Observer for BoundaryRecorder {
        fn observe(&self, _execution: &str, _section: &str, _event: Observation) {
            self.snapshots
                .lock()
                .expect("the snapshot mutex must not be poisoned")
                .push(self.store.glob("**").expect("the memory store can glob"));
        }
    }

    fn run(source: &str, args: &str) -> Result<LuaOutcome> {
        run_chunk(
            source,
            args,
            &json!({ "id": 1, "when": "t" }),
            &StoreRef::memory(),
            EXECUTION,
            &NullObserver,
            "Test",
        )
    }

    /// Run a chunk against a caller-supplied store, so a test can inspect the
    /// store after the chunk has run.
    fn run_with(source: &str, store: &StoreRef) -> Result<LuaOutcome> {
        run_chunk(
            source,
            "",
            &json!({ "id": 1, "when": "t" }),
            store,
            EXECUTION,
            &NullObserver,
            "Test",
        )
    }

    fn program(source: &str) -> LuaProgram {
        LuaProgram::compile(source, "test program", 1, EXECUTION, &NullObserver, "Test")
            .expect("test Lua must compile")
    }

    #[derive(Debug)]
    struct FixtureTool(&'static str);

    #[async_trait::async_trait]
    impl Tool for FixtureTool {
        fn id(&self) -> ToolId {
            ToolId::new("fixtures", self.0).expect("valid id")
        }

        fn wire_name(&self) -> &'static str {
            self.0
        }

        fn description(&self) -> &'static str {
            "fixture"
        }

        fn parameters_schema(&self) -> Json {
            json!({})
        }

        async fn call(&self, _arguments: Json) -> std::result::Result<ToolOutput, ToolError> {
            Ok(ToolOutput::trusted(String::new()))
        }
    }

    fn execute_live_tool_needs(
        source: &LuaProgram,
        resolver: &dyn ToolResolver,
        _execution: &str,
        _observer: &dyn Observer,
        _section: &str,
    ) -> Result<ToolBindings> {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(FixtureTool("search")),
            Arc::new(FixtureTool("fetch")),
        ];
        let registry = ToolRegistry::new(tools.iter().map(AsRef::as_ref));
        let models = |description: &str, _: &crate::model::ModelNeedOpts| {
            Err(Error::ModelAbsent {
                capability: description.to_owned(),
            })
        };
        let producer = LiveBindingProducer::default();
        let lua = Lua::new();
        harden(&lua)?;
        let result = lua.scope(|scope| {
            producer
                .install(&lua, scope, resolver, &registry, &models)
                .map_err(|error| mlua::Error::external(error.to_string()))?;
            lua.load(source.bytecode.as_slice()).exec()
        });
        if let Some(error) = producer.take_callback_error()? {
            return Err(error);
        }
        result.map_err(|error| Error::Lua(error.to_string()))?;
        producer.bindings().map(|(tools, _)| tools)
    }

    fn section_vm_with_bindings(
        _source: &LuaProgram,
        bindings: &ToolBindings,
        execution: &str,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<SectionVm> {
        SectionVm::new_for_section(
            None,
            bindings,
            &ModelBindings::default(),
            execution,
            observer,
            section,
        )
    }

    fn fixture_bindings(source: &str) -> (LuaProgram, ToolBindings) {
        let shared = program(source);
        let resolver = |description: &str| {
            Ok(ToolId::new(
                "fixtures",
                if description == "search the web" {
                    "search"
                } else {
                    "fetch"
                },
            )
            .expect("valid id"))
        };
        let bindings =
            execute_live_tool_needs(&shared, &resolver, EXECUTION, &NullObserver, "Prompt")
                .expect("fixture needs must resolve");
        (shared, bindings)
    }

    #[test]
    fn direct_output_is_absent_in_every_executable_lua_vm() {
        let library = program("assert(print == nil); assert(warn == nil); log('library load')");
        let library_vm = SectionVm::new(Some(&library), EXECUTION, &NullObserver, "Section")
            .expect("library VM must not expose direct output");
        library_vm.teardown(&NullObserver, "Section");

        let shared = program(
            "assert(print == nil)\n\
             assert(warn == nil)\n\
             tools.need('search', 'search the web')",
        );
        let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));
        let bindings =
            execute_live_tool_needs(&shared, &resolver, EXECUTION, &NullObserver, "Prompt")
                .expect("live H1 VM must not expose direct output");
        let mut vm =
            section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
                .expect("section VM must not expose direct output");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host must inject");
        vm.run_prologue(
            &program("assert(print == nil); assert(warn == nil)"),
            &NullObserver,
            "Section",
        )
        .expect("prologue must not expose direct output");
        vm.close_tool_scope(&NullObserver, "Section")
            .expect("scope must close");
        vm.run_epilog(
            &program("assert(print == nil); assert(warn == nil)"),
            &NullObserver,
            "Section",
        )
        .expect("epilog must not expose direct output");
        vm.teardown(&NullObserver, "Section");

        assert_eq!(
            run("return tostring(print) .. ':' .. tostring(warn)", "")
                .expect("compatibility VM must run")
                .returned
                .as_deref(),
            Some("nil:nil")
        );
    }

    #[test]
    fn logs_are_correlated_and_ordered_across_h2_phases() {
        let recorder = Recorder::default();
        let bindings = ToolBindings::for_test(
            vec![ToolBinding::for_test(
                "search",
                "search the web",
                ToolId::new("fixtures", "search").expect("valid id"),
            )],
            Vec::new(),
        );
        let mut vm =
            section_vm_with_bindings(&program(""), &bindings, EXECUTION, &recorder, "Gather")
                .expect("section VM must install captured bindings");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host must inject");
        vm.run_prologue(&program("log('prologue checkpoint')"), &recorder, "Gather")
            .expect("prologue log must succeed");
        vm.close_tool_scope(&recorder, "Gather")
            .expect("scope must close");
        vm.run_epilog(&program("log('epilog checkpoint')"), &recorder, "Gather")
            .expect("epilog log must succeed");
        vm.teardown(&recorder, "Gather");

        let details = recorder
            .records()
            .into_iter()
            .map(|(_, _, detail)| detail.to_string())
            .collect::<Vec<_>>();
        assert!(details.contains(&"Lua: prologue checkpoint".to_owned()));
        assert!(details.contains(&"Lua: epilog checkpoint".to_owned()));
        assert!(
            !details
                .iter()
                .any(|detail| detail.contains("binding") || detail.contains("replay"))
        );
    }

    #[test]
    fn compatibility_chunk_logs_interleave_with_host_operations() {
        let recorder = Recorder::default();
        run_chunk(
            "log('before write')\n\
             store.write('state.txt', 'value')\n\
             log('after write')",
            "",
            &json!({}),
            &StoreRef::memory(),
            "compatibility-run",
            &recorder,
            "Compatibility",
        )
        .expect("compatibility logging must succeed");

        assert_eq!(
            recorder.records(),
            [
                (
                    "compatibility-run".to_owned(),
                    "Compatibility".to_owned(),
                    Observation::Lua("before write".to_owned()),
                ),
                (
                    "compatibility-run".to_owned(),
                    "Compatibility".to_owned(),
                    detail::STORE_WRITE_SUCCEEDED.clone(),
                ),
                (
                    "compatibility-run".to_owned(),
                    "Compatibility".to_owned(),
                    Observation::Lua("after write".to_owned()),
                ),
            ]
        );
    }

    #[test]
    fn log_accepts_exactly_one_bounded_control_free_utf8_string() {
        let invalid = [
            ("log()", "log expects exactly one argument"),
            ("log('one', 'two')", "log expects exactly one argument"),
            ("log(42)", "log message must be a UTF-8 string"),
            (
                "log(string.char(255))",
                "log message must be a UTF-8 string",
            ),
            (
                "log('first\\nsecond')",
                "log message must not contain newline or control characters",
            ),
            (
                "log('first\\tsecond')",
                "log message must not contain newline or control characters",
            ),
            (
                "log('first\u{2028}second')",
                "log message must not contain newline or control characters",
            ),
        ];
        for (source, expected) in invalid {
            let recorder = Recorder::default();
            let error = run_chunk(
                source,
                "",
                &json!({}),
                &StoreRef::memory(),
                EXECUTION,
                &recorder,
                "Validation",
            )
            .expect_err("invalid log input must fail");
            assert!(
                error.to_string().contains(expected),
                "wrong validation error for {source:?}: {error}"
            );
            assert!(
                recorder.records().is_empty(),
                "invalid log input must emit no report"
            );
        }

        let too_long = "é".repeat(LUA_LOG_CHARACTER_LIMIT + 1);
        let source = format!(
            "log({})",
            serde_json::to_string(&too_long).expect("test string must serialize")
        );
        let error = run(&source, "").expect_err("257 characters must fail");
        assert!(
            error
                .to_string()
                .contains("log message must be at most 256 characters")
        );

        let maximum = "é".repeat(LUA_LOG_CHARACTER_LIMIT);
        let source = format!(
            "log({})",
            serde_json::to_string(&maximum).expect("test string must serialize")
        );
        let recorder = Recorder::default();
        run_chunk(
            &source,
            "",
            &json!({}),
            &StoreRef::memory(),
            EXECUTION,
            &recorder,
            "Validation",
        )
        .expect("256 Unicode characters must succeed");
        assert_eq!(
            recorder.records(),
            [(
                EXECUTION.to_owned(),
                "Validation".to_owned(),
                Observation::Lua(maximum.clone()),
            )]
        );
    }

    #[test]
    fn logging_does_not_change_results_or_store_effects_with_null_observer() {
        let source = "log('checkpoint')\n\
                      var.answer = args\n\
                      store.write('answer.txt', args)\n\
                      return var.answer";
        let recorded_store = StoreRef::memory();
        let recorder = Recorder::default();
        let observed_outcome = run_chunk(
            source,
            "same",
            &json!({}),
            &recorded_store,
            EXECUTION,
            &recorder,
            "Equivalence",
        )
        .expect("recorded execution must succeed");
        let null_store = StoreRef::memory();
        let silent = run_chunk(
            source,
            "same",
            &json!({}),
            &null_store,
            EXECUTION,
            &NullObserver,
            "Equivalence",
        )
        .expect("silent execution must succeed");

        assert_eq!(observed_outcome.returned, silent.returned);
        assert_eq!(observed_outcome.var, silent.var);
        assert_eq!(
            recorded_store
                .read("answer.txt")
                .expect("recorded write must persist"),
            null_store
                .read("answer.txt")
                .expect("silent write must persist")
        );
    }

    #[test]
    fn retained_log_functions_expire_with_their_phase_observer() {
        struct DropRecorder {
            dropped: Arc<std::sync::atomic::AtomicBool>,
            records: Arc<Mutex<Vec<(String, String, Observation)>>>,
        }

        impl Observer for DropRecorder {
            fn observe(&self, execution: &str, section: &str, event: Observation) {
                self.records
                    .lock()
                    .expect("the recorder mutex must not be poisoned")
                    .push((execution.to_owned(), section.to_owned(), event));
            }
        }

        impl Drop for DropRecorder {
            fn drop(&mut self) {
                self.dropped
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let first_records = Arc::new(Mutex::new(Vec::new()));
        let first = DropRecorder {
            dropped: Arc::clone(&dropped),
            records: Arc::clone(&first_records),
        };
        let mut vm =
            SectionVm::new(None, EXECUTION, &NullObserver, "Section").expect("VM must construct");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host must inject");
        vm.run_prologue(
            &program("saved_log = log; log('first phase')"),
            &first,
            "Section",
        )
        .expect("first phase log must succeed");
        drop(first);
        assert!(
            dropped.load(std::sync::atomic::Ordering::SeqCst),
            "the phase must not retain its observer"
        );

        vm.close_tool_scope(&NullObserver, "Section")
            .expect("scope must close before the epilog");
        let second = Recorder::default();
        vm.run_epilog(
            &program(
                "local ok = pcall(saved_log, 'stale callback')\n\
                 if ok then error('retained log callback remained live') end\n\
                 log('second phase')",
            ),
            &second,
            "Section",
        )
        .expect("a fresh epilog callback must replace the expired callback");
        vm.teardown(&second, "Section");

        assert_eq!(
            *first_records
                .lock()
                .expect("the recorder mutex must not be poisoned"),
            [
                (
                    EXECUTION.to_owned(),
                    "Section".to_owned(),
                    detail::LUA_PROLOGUE_STARTED.clone(),
                ),
                (
                    EXECUTION.to_owned(),
                    "Section".to_owned(),
                    Observation::Lua("first phase".to_owned()),
                ),
                (
                    EXECUTION.to_owned(),
                    "Section".to_owned(),
                    detail::LUA_PROLOGUE_SUCCEEDED.clone(),
                ),
            ]
        );
        assert!(
            second
                .records()
                .iter()
                .any(|(_, _, detail)| detail.to_string() == "Lua: second phase")
        );
        assert!(
            second
                .records()
                .iter()
                .all(|(_, _, detail)| detail.to_string() != "Lua: stale callback")
        );
    }

    #[test]
    fn concurrent_logs_keep_execution_ids_and_local_order() {
        let recorder = Arc::new(Recorder::default());
        let mut workers = Vec::new();
        for execution in ["execution-a", "execution-b"] {
            let recorder = Arc::clone(&recorder);
            workers.push(std::thread::spawn(move || {
                run_chunk(
                    "log('first'); log('second')",
                    "",
                    &json!({}),
                    &StoreRef::memory(),
                    execution,
                    recorder.as_ref(),
                    "Concurrent",
                )
                .expect("concurrent log run must succeed");
            }));
        }
        for worker in workers {
            worker.join().expect("logging worker must finish");
        }

        let records = recorder.records();
        for execution in ["execution-a", "execution-b"] {
            assert_eq!(
                records
                    .iter()
                    .filter(|(actual, _, _)| actual == execution)
                    .map(|(_, section, detail)| (section.clone(), detail.to_string()))
                    .collect::<Vec<_>>(),
                [
                    ("Concurrent".to_owned(), "Lua: first".to_owned()),
                    ("Concurrent".to_owned(), "Lua: second".to_owned()),
                ]
            );
        }
    }

    #[test]
    fn binding_records_exact_aliases_descriptions_identities_and_always_scope() {
        let source = "tools.need('web_search', 'search the web')\n\
                      tools.need('web_fetch2', 'fetch a page')\n\
                      tools.always('web_search')";
        let (_, bindings) = fixture_bindings(source);

        assert_eq!(
            bindings
                .bindings()
                .iter()
                .map(|binding| (binding.alias(), binding.description(), binding.id().name()))
                .collect::<Vec<_>>(),
            [
                ("web_search", "search the web", "search"),
                ("web_fetch2", "fetch a page", "fetch"),
            ]
        );
        assert_eq!(bindings.always(), ["web_search"]);
    }

    #[test]
    fn tool_need_returns_inspectable_object() {
        let shared = program(
            "local tool = tools.need('search', 'search the web')\n\
             assert(tool.name == 'search')\n\
             assert(tool.description == 'search the web')\n\
             assert(type(tool.parameters) == 'table')\n\
             assert(tool.wire_name == 'search')\n\
             assert(tool.untrusted == false)\n\
             tools.always('search')",
        );
        let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));
        let bindings =
            execute_live_tool_needs(&shared, &resolver, EXECUTION, &NullObserver, "Prompt")
                .expect("tools.need must return an inspectable Tool object");
        assert_eq!(bindings.bindings()[0].alias(), "search");

        let vm = section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
            .expect("section install must expose the same inspectable Tool object");
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn binding_validates_aliases_exactly() {
        let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));

        for alias in [
            "",
            "_leading",
            "has.dot",
            "nonasciié",
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-a",
        ] {
            let need = program(&format!("tools.need({alias:?}, 'capability')"));
            let error =
                execute_live_tool_needs(&need, &resolver, EXECUTION, &NullObserver, "Prompt")
                    .expect_err("invalid aliases must be rejected");
            assert!(
                error.to_string().contains("invalid tool alias"),
                "wrong error for {alias:?}: {error}"
            );
        }

        for valid in ["Upper", "has-dash", &format!("A{}", "2".repeat(63))] {
            let need = program(&format!("tools.need({valid:?}, 'capability')"));
            execute_live_tool_needs(&need, &resolver, EXECUTION, &NullObserver, "Prompt")
                .expect("planned alias forms must be valid");
        }
    }

    #[test]
    fn live_h1_rejects_duplicate_aliases() {
        let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));
        let error = execute_live_tool_needs(
            &program("tools.need('search', 'one'); tools.need('search', 'two')"),
            &resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .expect_err("duplicate aliases must fail");
        assert!(matches!(
            error,
            Error::DuplicateAlias { alias } if alias == "search"
        ));
    }

    #[test]
    fn duplicate_alias_error_cannot_be_suppressed_with_lua_pcall() {
        let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));
        let error = execute_live_tool_needs(
            &program("tools.need('search', 'one'); pcall(tools.need, 'search', 'two')"),
            &resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .expect_err("a caught duplicate callback must still fail binding");
        assert!(matches!(
            error,
            Error::DuplicateAlias { alias } if alias == "search"
        ));
    }

    #[test]
    fn binding_rejects_unknown_and_duplicate_always_aliases() {
        let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));
        for source in [
            "tools.always('missing')",
            "tools.need('search', 'one'); tools.always('search'); tools.always('search')",
        ] {
            let error = execute_live_tool_needs(
                &program(source),
                &resolver,
                EXECUTION,
                &NullObserver,
                "Prompt",
            )
            .expect_err("invalid always declarations must fail");
            assert!(
                error.to_string().contains("not declared")
                    || error.to_string().contains("more than once")
            );
        }
    }

    #[test]
    fn captured_bindings_do_not_execute_h1_source() {
        let (_, bindings) =
            fixture_bindings("tools.need('search', 'search the web'); tools.always('search')");
        let mut vm = section_vm_with_bindings(
            &program("h1_was_executed = true"),
            &bindings,
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .expect("captured bindings must install without executing H1");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host must inject");
        vm.run_prologue(
            &program("assert(h1_was_executed == nil); tools.add('search')"),
            &NullObserver,
            "Section",
        )
        .expect("captured binding must be available without H1 execution");
    }

    #[test]
    fn h2_recording_closes_to_always_then_added_scope() {
        let (shared, bindings) = fixture_bindings(
            "tools.need('search', 'search the web'); \
             tools.need('fetch', 'fetch a page'); \
             tools.always('search')",
        );
        let prologue = program("tools.add('fetch', 'search', 'fetch')");
        let mut vm =
            section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
                .expect("captured bindings must install");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host must inject");
        vm.run_prologue(&prologue, &NullObserver, "Section")
            .expect("H2 additions must record");
        let scope = vm
            .close_tool_scope(&NullObserver, "Section")
            .expect("scope must close");

        assert_eq!(
            scope
                .bindings()
                .iter()
                .map(ToolBinding::alias)
                .collect::<Vec<_>>(),
            ["search", "fetch"]
        );
        assert!(
            vm.close_tool_scope(&NullObserver, "Section").is_err(),
            "scope closure must be one-way"
        );
        let error = vm
            .run_epilog(&program("tools.add('fetch')"), &NullObserver, "Section")
            .expect_err("epilogs cannot mutate a closed scope");
        assert!(error.to_string().contains("scope closes"));
    }

    #[test]
    fn h2_add_accepts_tool_objects_and_arrays() {
        let resolver = |description: &str| {
            Ok(ToolId::new(
                "fixtures",
                if description == "search the web" {
                    "search"
                } else {
                    "fetch"
                },
            )
            .expect("valid id"))
        };
        let h1_error = execute_live_tool_needs(
            &program(
                "local search = tools.need('search', 'search the web'); \
                 tools.add(search)",
            ),
            &resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .expect_err("tools.add must stay H2-only even when passed a Tool object");
        assert!(
            h1_error
                .to_string()
                .contains("tools.add is only available during H2 recording"),
            "H1 tools.add(Tool) must report the phase error, not a type error: {h1_error}"
        );

        let (shared, bindings) = fixture_bindings(
            "search = tools.need('search', 'search the web'); \
             fetch = tools.need('fetch', 'fetch a page')",
        );
        let prologue = program(
            "tools.add(search); \
             tools.add({fetch}); \
             tools.add(search, 'fetch', {search, fetch})",
        );
        let mut vm =
            section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
                .expect("captured bindings must install");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host must inject");
        vm.run_prologue(&prologue, &NullObserver, "Section")
            .expect("tools.add must accept Tool objects, strings, and arrays");
        let scope = vm
            .close_tool_scope(&NullObserver, "Section")
            .expect("scope must close");

        assert_eq!(
            scope
                .bindings()
                .iter()
                .map(ToolBinding::alias)
                .collect::<Vec<_>>(),
            ["search", "fetch"]
        );
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn empty_add_is_a_no_op_and_failed_variadic_add_is_atomic() {
        let (shared, bindings) = fixture_bindings(
            "tools.need('search', 'search the web'); \
             tools.need('fetch', 'fetch a page')",
        );
        let prologue = program(
            "tools.add(); \
             local ok = pcall(tools.add, 'search', 'missing'); \
             if ok then error('invalid add unexpectedly succeeded') end; \
             tools.add('fetch')",
        );
        let mut vm =
            section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
                .expect("captured bindings must install");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host must inject");
        vm.run_prologue(&prologue, &NullObserver, "Section")
            .expect("caught failed add must not poison recording");
        let scope = vm
            .close_tool_scope(&NullObserver, "Section")
            .expect("scope must close");

        assert_eq!(
            scope
                .bindings()
                .iter()
                .map(ToolBinding::alias)
                .collect::<Vec<_>>(),
            ["fetch"],
            "empty add changes nothing and failed add records no partial aliases"
        );
    }

    #[test]
    fn bound_reply_and_epilog_require_closed_scope() {
        let (shared, bindings) = fixture_bindings("tools.need('search', 'search the web')");
        let mut vm =
            section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
                .expect("captured bindings must install");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host must inject");

        let reply_error = vm
            .bind_reply("answer", &NullObserver, "Section")
            .expect_err("reply binding must not bypass scope closure");
        assert!(reply_error.to_string().contains("scope must close"));
        let epilog_error = vm
            .run_epilog(&program("return reply"), &NullObserver, "Section")
            .expect_err("epilog must not bypass scope closure");
        assert!(epilog_error.to_string().contains("scope must close"));

        vm.close_tool_scope(&NullObserver, "Section")
            .expect("scope must close");
        vm.bind_reply("answer", &NullObserver, "Section")
            .expect("reply may bind after closure");
        assert_eq!(
            vm.run_epilog(&program("return reply"), &NullObserver, "Section")
                .expect("epilog may run after closure")
                .as_deref(),
            Some("answer")
        );
    }

    #[test]
    fn tool_operations_enforce_their_lifecycle_phase_even_when_captured() {
        let (shared, bindings) = fixture_bindings("tools.need('search', 'search the web')");
        let mut vm =
            section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
                .expect("captured bindings must install");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host must inject");

        let error = vm
            .run_prologue(
                &program("tools.need('other', 'fetch a page')"),
                &NullObserver,
                "Section",
            )
            .expect_err("current H2 table must reject need");
        assert!(
            error
                .to_string()
                .contains("only available during live H1 execution")
        );
    }

    #[test]
    fn unknown_h2_alias_fails_before_scope_closure() {
        let (shared, bindings) = fixture_bindings("tools.need('search', 'search the web')");
        let mut vm =
            section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
                .expect("captured bindings must install");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host must inject");
        let error = vm
            .run_prologue(&program("tools.add('missing')"), &NullObserver, "Section")
            .expect_err("only declared aliases may enter H2 scope");
        assert!(error.to_string().contains("not declared"));
    }

    #[test]
    fn captured_bindings_are_installed_without_payload_reports() {
        let bindings = ToolBindings::for_test(
            vec![ToolBinding::for_test(
                "private_alias",
                "private capability",
                ToolId::new("fixtures", "search").expect("valid id"),
            )],
            Vec::new(),
        );
        let recorder = Recorder::default();
        let mut vm =
            section_vm_with_bindings(&program(""), &bindings, EXECUTION, &recorder, "Section")
                .expect("captured binding installation must succeed");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host must inject");
        vm.close_tool_scope(&recorder, "Section")
            .expect("empty H2 scope must close");
        let trace = format!("{:?}", recorder.observations());
        assert!(!trace.contains("private_alias"));
        assert!(!trace.contains("private capability"));
    }

    #[test]
    fn scope_closure_reports_exact_payload_free_sequence() {
        let recorder = Recorder::default();
        let vm = SectionVm::new(None, EXECUTION, &recorder, "private section")
            .expect("VM must construct");
        let scope = vm
            .close_tool_scope(&recorder, "private section")
            .expect("empty scope may close before host injection");
        assert!(scope.bindings().is_empty());

        assert_eq!(
            recorder.observations(),
            [
                (
                    "private section".to_owned(),
                    detail::TOOL_SCOPE_CLOSING.clone(),
                ),
                (
                    "private section".to_owned(),
                    detail::TOOL_SCOPE_CLOSED.clone(),
                ),
                (
                    "private section".to_owned(),
                    detail::MODEL_SCOPE_CLOSING.clone(),
                ),
                (
                    "private section".to_owned(),
                    detail::MODEL_SCOPE_CLOSED.clone(),
                ),
            ]
        );
    }

    #[test]
    fn section_vm_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<SectionVm>();
    }

    #[test]
    fn section_vm_preserves_one_environment_across_all_phases() {
        let shared = program(
            "shared_saw_args = args\n\
             function decorate(value) return '<' .. value .. '>' end",
        );
        let prologue = program(
            "var.from_shared = decorate(args)\n\
             store.write('phase.txt', var.from_shared)",
        );
        let epilog =
            program("return shared_saw_args == nil and decorate(reply) or 'host leaked early'");
        let store = StoreRef::memory();
        let mut vm = SectionVm::new(Some(&shared), EXECUTION, &NullObserver, "Test")
            .expect("shared program must run");
        vm.inject_host("input", &json!({ "id": 7 }), &store, None)
            .expect("host values must inject");

        assert_eq!(
            vm.run_prologue(&prologue, &NullObserver, "Test")
                .expect("prologue must run"),
            None
        );
        assert_eq!(
            vm.var()
                .expect("var must serialize")
                .get("from_shared")
                .and_then(Json::as_str),
            Some("<input>")
        );
        assert_eq!(
            store
                .read_lines("phase.txt")
                .expect("shared store must read_lines"),
            "1| <input>"
        );

        vm.close_tool_scope(&NullObserver, "Test")
            .expect("scope must close");
        vm.bind_reply("model answer", &NullObserver, "Test")
            .expect("reply must bind into the same environment");
        assert_eq!(
            vm.run_epilog(&epilog, &NullObserver, "Test")
                .expect("epilog must run")
                .as_deref(),
            Some("<model answer>")
        );
    }

    #[test]
    fn section_vm_requires_delayed_single_host_injection() {
        let no_op = program("return args");
        let store = StoreRef::memory();
        let mut vm = SectionVm::new(None, EXECUTION, &NullObserver, "Test").expect("VM must build");

        let error = vm
            .run_prologue(&no_op, &NullObserver, "Test")
            .expect_err("programs cannot run before host injection");
        assert!(error.to_string().contains("not been injected"));

        vm.inject_host("first", &json!({}), &store, None)
            .expect("first injection must succeed");
        let error = vm
            .inject_host("second", &json!({}), &store, None)
            .expect_err("host values cannot be replaced");
        assert!(error.to_string().contains("already injected"));
    }

    #[test]
    fn section_vm_host_injection_bypasses_shared_global_metatables() {
        let shared = program(
            "captured = {}\n\
             setmetatable(_G, { __newindex = function(_, key, value) captured[key] = value end })",
        );
        let inspect = program(
            "return tostring(captured.args) .. ',' .. tostring(captured.store) .. ',' .. args",
        );
        let mut vm = SectionVm::new(Some(&shared), EXECUTION, &NullObserver, "Test")
            .expect("shared program must run");
        vm.inject_host("private input", &json!({}), &StoreRef::memory(), None)
            .expect("raw host injection must bypass the shared metatable");

        assert_eq!(
            vm.run_prologue(&inspect, &NullObserver, "Test")
                .expect("inspection must run")
                .as_deref(),
            Some("nil,nil,private input")
        );
    }

    #[test]
    fn section_vm_reports_store_operations_in_each_phase() {
        let write = program("store.write('state.txt', args)");
        let read = program("return store.read_lines('state.txt')");
        let recorder = Recorder::default();
        let mut vm =
            SectionVm::new(None, EXECUTION, &NullObserver, "Gather").expect("VM must build");
        vm.inject_host("private input", &json!({}), &StoreRef::memory(), None)
            .expect("host values must inject");

        vm.run_prologue(&write, &recorder, "Gather")
            .expect("prologue write must run");
        vm.close_tool_scope(&recorder, "Gather")
            .expect("scope must close");
        vm.bind_reply("private reply", &recorder, "Gather")
            .expect("reply must bind");
        vm.run_epilog(&read, &recorder, "Gather")
            .expect("epilog read must run");
        vm.teardown(&recorder, "Gather");

        assert_eq!(
            recorder.observations(),
            vec![
                ("Gather".to_owned(), detail::LUA_PROLOGUE_STARTED.clone(),),
                ("Gather".to_owned(), detail::STORE_WRITE_SUCCEEDED.clone(),),
                ("Gather".to_owned(), detail::LUA_PROLOGUE_SUCCEEDED.clone(),),
                ("Gather".to_owned(), detail::TOOL_SCOPE_CLOSING.clone(),),
                ("Gather".to_owned(), detail::TOOL_SCOPE_CLOSED.clone(),),
                ("Gather".to_owned(), detail::MODEL_SCOPE_CLOSING.clone(),),
                ("Gather".to_owned(), detail::MODEL_SCOPE_CLOSED.clone(),),
                (
                    "Gather".to_owned(),
                    detail::LUA_REPLY_BINDING_STARTED.clone(),
                ),
                (
                    "Gather".to_owned(),
                    detail::LUA_REPLY_BINDING_SUCCEEDED.clone(),
                ),
                ("Gather".to_owned(), detail::LUA_EPILOG_STARTED.clone(),),
                (
                    "Gather".to_owned(),
                    detail::STORE_READ_LINES_SUCCEEDED.clone(),
                ),
                ("Gather".to_owned(), detail::LUA_EPILOG_SUCCEEDED.clone(),),
                ("Gather".to_owned(), detail::LUA_TEARDOWN_STARTED.clone(),),
                ("Gather".to_owned(), detail::LUA_TEARDOWN_SUCCEEDED.clone(),),
            ]
        );
        let trace = format!("{:?}", recorder.observations());
        assert!(!trace.contains("private input"));
        assert!(!trace.contains("private reply"));
        assert!(!trace.contains("state.txt"));
    }

    #[test]
    fn section_vm_accepts_only_scalar_top_level_returns() {
        let store = StoreRef::memory();
        for (source, expected) in [
            ("return 'text'", Some("text")),
            ("return 42", Some("42")),
            ("return 1.5", Some("1.5")),
            ("return true", Some("true")),
            ("return nil", None),
        ] {
            let mut vm =
                SectionVm::new(None, EXECUTION, &NullObserver, "Test").expect("VM must build");
            vm.inject_host("", &json!({}), &store, None)
                .expect("host values must inject");
            assert_eq!(
                vm.run_prologue(&program(source), &NullObserver, "Test")
                    .expect("scalar return must work")
                    .as_deref(),
                expected
            );
        }

        let mut vm = SectionVm::new(None, EXECUTION, &NullObserver, "Test").expect("VM must build");
        vm.inject_host("", &json!({}), &store, None)
            .expect("host values must inject");
        let error = vm
            .run_prologue(&program("return {}"), &NullObserver, "Test")
            .expect_err("table returns must be refused");
        assert!(error.to_string().contains("cannot return a table"));
    }

    #[test]
    fn section_vms_isolate_mutated_shared_globals() {
        let shared = program("counter = 0");
        let increment = program("counter = counter + 1; return counter");
        let store = StoreRef::memory();
        let mut first = SectionVm::new(Some(&shared), EXECUTION, &NullObserver, "First")
            .expect("first VM must build");
        let mut second = SectionVm::new(Some(&shared), EXECUTION, &NullObserver, "Second")
            .expect("second VM must build");
        first
            .inject_host("", &json!({}), &store, None)
            .expect("first host must inject");
        second
            .inject_host("", &json!({}), &store, None)
            .expect("second host must inject");

        assert_eq!(
            first
                .run_prologue(&increment, &NullObserver, "First")
                .expect("first increment must run")
                .as_deref(),
            Some("1")
        );
        first
            .close_tool_scope(&NullObserver, "First")
            .expect("first scope must close");
        assert_eq!(
            first
                .run_epilog(&increment, &NullObserver, "First")
                .expect("second first-VM increment must run")
                .as_deref(),
            Some("2")
        );
        assert_eq!(
            second
                .run_prologue(&increment, &NullObserver, "Second")
                .expect("second VM increment must run")
                .as_deref(),
            Some("1")
        );
    }

    #[test]
    fn shared_program_consumes_the_later_phase_instruction_budget() {
        let work = program("for i = 1, 3000000 do local value = i end");
        let mut vm = SectionVm::new(Some(&work), EXECUTION, &NullObserver, "Test")
            .expect("shared work must fit the budget");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host values must inject");

        let error = vm
            .run_prologue(&work, &NullObserver, "Test")
            .expect_err("the prologue must exhaust the budget left by shared execution");
        assert!(error.to_string().contains("instruction budget exceeded"));
    }

    #[test]
    fn section_lifecycle_reports_are_ordered_exact_and_payload_free() {
        let shared = program("private_global = 'shared secret'");
        let prologue = program("var.value = args");
        let epilog = program("return reply");
        let recorder = Recorder::default();
        let mut vm = SectionVm::new(Some(&shared), EXECUTION, &recorder, "Gather")
            .expect("shared program must run");
        vm.inject_host("private input", &json!({}), &StoreRef::memory(), None)
            .expect("host values must inject");
        vm.run_prologue(&prologue, &recorder, "Gather")
            .expect("prologue must run");
        vm.close_tool_scope(&recorder, "Gather")
            .expect("scope must close");
        vm.bind_reply("private reply", &recorder, "Gather")
            .expect("reply must bind");
        vm.run_epilog(&epilog, &recorder, "Gather")
            .expect("epilog must run");
        vm.teardown(&recorder, "Gather");

        let observations = recorder.observations();
        assert_eq!(
            observations,
            [
                detail::LUA_SHARED_LOAD_STARTED,
                detail::LUA_SHARED_LOAD_SUCCEEDED,
                detail::LUA_PROLOGUE_STARTED,
                detail::LUA_PROLOGUE_SUCCEEDED,
                detail::TOOL_SCOPE_CLOSING,
                detail::TOOL_SCOPE_CLOSED,
                detail::MODEL_SCOPE_CLOSING,
                detail::MODEL_SCOPE_CLOSED,
                detail::LUA_REPLY_BINDING_STARTED,
                detail::LUA_REPLY_BINDING_SUCCEEDED,
                detail::LUA_EPILOG_STARTED,
                detail::LUA_EPILOG_SUCCEEDED,
                detail::LUA_TEARDOWN_STARTED,
                detail::LUA_TEARDOWN_SUCCEEDED,
            ]
            .into_iter()
            .map(|detail| ("Gather".to_owned(), detail.clone()))
            .collect::<Vec<_>>()
        );
        let trace = format!("{observations:?}");
        assert!(!trace.contains("shared secret"));
        assert!(!trace.contains("private input"));
        assert!(!trace.contains("private reply"));
    }

    #[test]
    fn section_lifecycle_failures_report_their_phase() {
        let recorder = Recorder::default();
        let failing_shared = program("error('private shared failure')");
        SectionVm::new(Some(&failing_shared), EXECUTION, &recorder, "Shared")
            .expect_err("shared execution must fail");
        assert_eq!(
            recorder.observations(),
            [
                detail::LUA_SHARED_LOAD_STARTED,
                detail::LUA_SHARED_LOAD_FAILED,
                detail::LUA_TEARDOWN_STARTED,
                detail::LUA_TEARDOWN_SUCCEEDED,
            ]
            .into_iter()
            .map(|detail| ("Shared".to_owned(), detail.clone()))
            .collect::<Vec<_>>()
        );

        let recorder = Recorder::default();
        let vm = SectionVm::new(None, EXECUTION, &NullObserver, "Prologue").expect("VM must build");
        vm.run_prologue(&program("return nil"), &recorder, "Prologue")
            .expect_err("prologue before injection must fail");
        assert!(
            recorder
                .observations()
                .iter()
                .any(|(_, event)| *event == detail::LUA_PROLOGUE_FAILED)
        );
    }

    #[test]
    fn lua_program_retains_source_and_round_trips_bytecode() {
        let source = "return greeting .. ' world'";
        let program = LuaProgram::compile(
            source,
            "section Gather prologue",
            1,
            EXECUTION,
            &NullObserver,
            "Gather",
        )
        .expect("valid Lua must compile");
        assert_eq!(program.source(), source);

        for greeting in ["hello", "goodbye"] {
            let lua = Lua::new();
            lua.globals()
                .set("greeting", greeting)
                .expect("the test global must install");
            let function = program.load(&lua).expect("bytecode must load");
            let returned: String = function.call(()).expect("bytecode must execute");
            assert_eq!(returned, format!("{greeting} world"));
        }
    }

    #[test]
    fn runtime_assert_failure_reports_chunk_name_and_line() {
        let location = "section `Web Search` epilog";
        let program = LuaProgram::compile(
            "local x = 1\nassert(false)\nreturn x",
            location,
            1,
            EXECUTION,
            &NullObserver,
            "Web Search",
        )
        .expect("valid Lua must compile");
        let lua = Lua::new();
        let function = program.load(&lua).expect("bytecode must load");
        let error = function
            .call::<()>(())
            .expect_err("assert(false) must fail at runtime");
        let message = error.to_string();
        assert!(
            message.contains(location),
            "runtime error must name the chunk: {message}"
        );
        assert!(
            message.contains(":2:") || message.contains(":2\n"),
            "runtime error must include the failing line number: {message}"
        );
        assert!(
            !message.contains("?:"),
            "stripped debug info must not leave '?:' in the traceback: {message}"
        );
    }

    #[test]
    fn map_chunk_line_to_absolute_rewrites_line_numbers() {
        let location = "section `Web Search` epilog";
        let msg = r#"[string "section `Web Search` epilog"]:2: assertion failed!"#;
        let result = map_chunk_line_to_absolute(msg, 50, location);
        assert_eq!(
            result,
            r#"section `Web Search` epilog:51: [string "section `Web Search` epilog"]:51: assertion failed!"#
        );
    }

    #[test]
    fn map_chunk_line_to_absolute_only_rewrites_matching_chunk() {
        let msg = r#"[string "section `Web Search` epilog"]:51: assertion failed!
stack traceback:
        [string "section `Main` prologue"]:3: in main chunk"#;
        let result = map_chunk_line_to_absolute(msg, 22, "section `Main` prologue");
        assert!(
            result.contains("[string \"section `Web Search` epilog\"]:51:"),
            "child absolute line must stay intact: {result}"
        );
        assert!(
            result.contains("[string \"section `Main` prologue\"]:24:")
                || result.starts_with("section `Main` prologue:24:"),
            "parent chunk line must map with parent source_line: {result}"
        );
        assert!(
            !result.contains("[string \"section `Main` prologue\"]:3:"),
            "parent chunk-relative line must be rewritten: {result}"
        );
    }

    #[test]
    fn map_chunk_line_to_absolute_passthrough_when_source_line_zero() {
        let msg = r#"[string "x"]:5: boom"#;
        let result = map_chunk_line_to_absolute(msg, 0, "x");
        assert_eq!(result, msg);
    }

    #[test]
    fn map_chunk_line_to_absolute_no_match_passthrough() {
        let msg = "some other error without chunk info";
        let result = map_chunk_line_to_absolute(msg, 10, "section `Main` prologue");
        assert_eq!(result, msg);
    }

    #[test]
    fn runtime_error_maps_to_absolute_prompt_line() {
        let location = "section `Web Search` epilog";
        let source_line: u32 = 50;
        let program = LuaProgram::compile(
            "local x = 1\nassert(false)\nreturn x",
            location,
            source_line,
            EXECUTION,
            &NullObserver,
            "Web Search",
        )
        .expect("valid Lua must compile");

        let lua = Lua::new();
        let function = program.load(&lua).expect("bytecode must load");
        let raw_error = function
            .call::<()>(())
            .expect_err("assert(false) must fail at runtime");

        let mapped = program.map_runtime_error(&raw_error);
        let msg = mapped.to_string();
        // chunk line 2 + source_line 50 - 1 = 51
        assert!(
            msg.contains(":51:"),
            "mapped error must contain absolute line 51: {msg}"
        );
        assert!(
            msg.contains(location),
            "mapped error must preserve the chunk name: {msg}"
        );
    }

    #[test]
    fn malformed_lua_reports_location_and_retains_source_diagnostic() {
        let source = "local secret =\nreturn secret";
        let location = "section Gather prologue";
        let error = LuaProgram::compile(source, location, 1, EXECUTION, &NullObserver, "Gather")
            .expect_err("malformed Lua must not compile");

        match &error {
            Error::LuaCompile {
                location: actual_location,
                lua_source: actual_source,
                message,
                ..
            } => {
                assert_eq!(actual_location, location);
                assert_eq!(actual_source, source);
                assert!(
                    message.contains(location),
                    "the Lua diagnostic must identify its source region: {message}"
                );
            }
            other => panic!("expected Error::LuaCompile, got {other:?}"),
        }
        assert!(
            error.to_string().contains(location),
            "the displayed error must identify its source region"
        );
    }

    #[test]
    fn lua_compilation_reports_are_ordered_exact_and_payload_free() {
        let recorder = Recorder::default();
        let source = "return 'private source payload'";
        let location = "private/location";
        LuaProgram::compile(source, location, 1, EXECUTION, &recorder, "Gather")
            .expect("valid Lua must compile");
        assert_eq!(
            recorder.observations(),
            vec![
                ("Gather".to_owned(), detail::LUA_COMPILATION_STARTED.clone(),),
                (
                    "Gather".to_owned(),
                    detail::LUA_COMPILATION_SUCCEEDED.clone(),
                ),
            ]
        );

        let recorder = Recorder::default();
        LuaProgram::compile(
            "local private =",
            location,
            1,
            EXECUTION,
            &recorder,
            "Gather",
        )
        .expect_err("malformed Lua must fail");
        let observations = recorder.observations();
        assert_eq!(
            observations,
            vec![
                ("Gather".to_owned(), detail::LUA_COMPILATION_STARTED.clone(),),
                ("Gather".to_owned(), detail::LUA_COMPILATION_FAILED.clone(),),
            ]
        );
        let trace = format!("{observations:?}");
        assert!(!trace.contains("private"));
        assert!(!trace.contains(location));
    }

    #[test]
    fn returns_args_verbatim() {
        assert_eq!(
            run("return args", "hello").unwrap().returned.as_deref(),
            Some("hello")
        );
    }

    #[test]
    fn expression_only_compatibility_chunk_returns_its_value() {
        assert_eq!(run("42", "").unwrap().returned.as_deref(), Some("42"));
    }

    #[test]
    fn no_return_is_none() {
        assert_eq!(run("local x = 1", "hello").unwrap().returned, None);
    }

    #[test]
    fn reads_sys() {
        assert_eq!(
            run("return sys.id", "").unwrap().returned.as_deref(),
            Some("1")
        );
        assert_eq!(
            run("return sys.when", "").unwrap().returned.as_deref(),
            Some("t")
        );
    }

    #[test]
    fn unknown_sys_field_is_a_lua_error() {
        let error = run("return sys.bogus", "").expect_err("missing sys field must fail");
        assert!(
            error.to_string().contains("unknown sys field 'bogus'"),
            "error was {error}"
        );
    }

    #[test]
    fn writing_sys_field_is_a_lua_error() {
        let existing =
            run("sys.when = 'x'", "").expect_err("writing an existing sys field must fail");
        assert!(
            existing
                .to_string()
                .contains("sys is read-only; cannot set 'when'"),
            "error was {existing}"
        );

        let created = run("sys.extra = 1", "").expect_err("creating a sys field must fail");
        assert!(
            created
                .to_string()
                .contains("sys is read-only; cannot set 'extra'"),
            "error was {created}"
        );
    }

    #[test]
    fn var_is_read_back() {
        let out = run("var.greeting = 'hi ' .. args", "bob").unwrap();
        assert_eq!(
            out.var.get("greeting").and_then(|v| v.as_str()),
            Some("hi bob")
        );
    }

    #[test]
    fn safe_stdlib_present() {
        let out = run("return string.upper(args)", "hi").unwrap();
        assert_eq!(out.returned.as_deref(), Some("HI"));
    }

    #[test]
    fn dangerous_globals_absent() {
        let out = run(
            "return tostring(io) .. ',' .. tostring(os) .. ',' .. tostring(require) .. ',' .. tostring(load)",
            "",
        )
        .unwrap();
        assert_eq!(out.returned.as_deref(), Some("nil,nil,nil,nil"));
    }

    #[test]
    fn instruction_budget_aborts_runaway() {
        assert!(run("while true do end", "").is_err());
    }

    #[test]
    fn add_without_declarations_fails_as_undeclared_in_a_chunk() {
        let error =
            run("tools.add('web_search')", "").expect_err("an undeclared alias must fail loudly");
        assert!(
            error
                .to_string()
                .contains("tools.add alias \"web_search\" was not declared by tools.need"),
            "the error must name the undeclared alias: {error}"
        );
    }

    #[test]
    fn add_without_declarations_fails_in_a_prologue_without_a_shared_library() {
        let mut vm = SectionVm::new(None, EXECUTION, &NullObserver, "Test").expect("VM must build");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host values must inject");
        let error = vm
            .run_prologue(&program("tools.add('web_search')"), &NullObserver, "Test")
            .expect_err("an undeclared alias must fail loudly");
        assert!(
            error.to_string().contains("not declared by tools.need"),
            "the error must report the missing declaration: {error}"
        );
        vm.teardown(&NullObserver, "Test");
    }

    #[test]
    fn add_with_empty_frozen_needs_fails_as_undeclared() {
        let shared = program("function helper() return 'no declarations' end");
        let resolver = |description: &str| -> Result<ToolId> {
            panic!("a declaration-free program must not resolve {description:?}")
        };
        let bindings =
            execute_live_tool_needs(&shared, &resolver, EXECUTION, &NullObserver, "Prompt")
                .expect("a need-free H1 program must execute");
        assert!(bindings.bindings().is_empty());
        let mut vm = section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Test")
            .expect("empty captured bindings must install");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host values must inject");
        let error = vm
            .run_prologue(&program("tools.add('web_search')"), &NullObserver, "Test")
            .expect_err("an undeclared alias must fail loudly");
        assert!(
            error.to_string().contains("not declared by tools.need"),
            "the error must report the missing declaration: {error}"
        );
        vm.teardown(&NullObserver, "Test");
    }

    #[test]
    fn add_with_a_description_argument_fails_alias_validation() {
        let (shared, bindings) = fixture_bindings("tools.need('search', 'search the web')");
        let mut vm = section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Test")
            .expect("captured bindings must install");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host values must inject");
        let error = vm
            .run_prologue(
                &program("tools.add('search', 'Search the web for pages matching a query.')"),
                &NullObserver,
                "Test",
            )
            .expect_err("a description passed to tools.add must fail alias validation");
        assert!(
            error.to_string().contains("invalid tool alias"),
            "the error must report the invalid alias: {error}"
        );
        vm.teardown(&NullObserver, "Test");
    }

    #[test]
    fn a_section_vm_without_declarations_closes_to_an_empty_scope() {
        let mut vm = SectionVm::new(None, EXECUTION, &NullObserver, "Test").expect("VM must build");
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .expect("host values must inject");
        let scope = vm
            .close_tool_scope(&NullObserver, "Test")
            .expect("an empty scope must close");
        assert!(scope.bindings().is_empty());
        vm.teardown(&NullObserver, "Test");
    }

    // --- The always-on `store` table ---

    #[test]
    fn store_exists_returns_boolean() {
        let store = StoreRef::memory();
        assert_eq!(
            run_with("return tostring(store.exists('missing.txt'))", &store)
                .unwrap()
                .returned
                .as_deref(),
            Some("false")
        );
        store.write("a.txt", "hi").expect("write");
        assert_eq!(
            run_with("return tostring(store.exists('a.txt'))", &store)
                .unwrap()
                .returned
                .as_deref(),
            Some("true")
        );
        assert_eq!(
            run_with(
                "store.delete('a.txt')\nreturn tostring(store.exists('a.txt'))",
                &store,
            )
            .unwrap()
            .returned
            .as_deref(),
            Some("false")
        );
    }

    #[test]
    fn store_write_then_read_lines_returns_numbered_content() {
        let out = run(
            "store.write('a.txt', 'first\\nsecond')\nreturn store.read_lines('a.txt')",
            "",
        )
        .unwrap();
        assert_eq!(out.returned.as_deref(), Some("1| first\n2| second"));
    }

    #[test]
    fn store_append_extends_the_file() {
        let out = run(
            "store.append('log.txt', 'one\\n')\nstore.append('log.txt', 'two')\nreturn store.read_lines('log.txt')",
            "",
        )
        .unwrap();
        assert_eq!(out.returned.as_deref(), Some("1| one\n2| two"));
    }

    #[test]
    fn store_str_replace_edits_in_place() {
        let out = run(
            "store.write('a.txt', 'the quick brown fox')\nstore.str_replace('a.txt', 'quick', 'slow')\nreturn store.read_lines('a.txt')",
            "",
        )
        .unwrap();
        assert_eq!(out.returned.as_deref(), Some("1| the slow brown fox"));
    }

    #[test]
    fn store_delete_then_read_raises() {
        let err = run(
            "store.write('a.txt', 'gone soon')\nstore.delete('a.txt')\nreturn store.read_lines('a.txt')",
            "",
        )
        .expect_err("reading a deleted file must raise");
        match err {
            Error::Lua(msg) => assert!(
                msg.contains("file not found"),
                "the Lua error must carry the store message, got: {msg}"
            ),
            other => panic!("expected Error::Lua, got {other:?}"),
        }
    }

    #[test]
    fn store_glob_returns_a_sorted_array() {
        let out = run(
            "store.write('src/b.rs', '')\nstore.write('src/a.rs', '')\nlocal g = store.glob('src/*.rs')\nreturn g[1] .. ',' .. g[2]",
            "",
        )
        .unwrap();
        assert_eq!(out.returned.as_deref(), Some("src/a.rs,src/b.rs"));
    }

    #[test]
    fn store_error_surfaces_as_lua_error() {
        // An ambiguous `str_replace` anchor is a `StoreError`, which must reach
        // the caller as `Error::Lua` (mapped through `mlua::Error::external`).
        let err = run(
            "store.write('a.txt', 'na na na')\nstore.str_replace('a.txt', 'na', 'la')",
            "",
        )
        .expect_err("an ambiguous anchor must raise");
        match err {
            Error::Lua(msg) => assert!(
                msg.contains("expected exactly one"),
                "the Lua error must carry the ambiguity message, got: {msg}"
            ),
            other => panic!("expected Error::Lua, got {other:?}"),
        }
    }

    #[test]
    fn store_writes_are_visible_on_the_shared_handle() {
        // The table is backed by the caller's handle, so a write from Lua is
        // observable through a clone of that same handle after the chunk ends.
        let store = StoreRef::memory();
        run_with("store.write('shared.txt', 'from lua')", &store).unwrap();
        assert_eq!(
            store.read_lines("shared.txt").expect("read_lines"),
            "1| from lua",
            "a Lua write must land in the shared store"
        );
    }

    #[test]
    fn store_reports_are_ordered_exact_and_payload_free_on_failure() {
        let recorder = Recorder::default();
        let store = StoreRef::memory();
        let source = "store.write('secret/path.txt', 'private contents')\n\
                      store.read_lines('secret/path.txt')\n\
                      store.str_replace('secret/path.txt', 'missing secret', 'replacement')";
        let error = run_chunk(
            source,
            "private input",
            &json!({ "id": 1, "when": "t" }),
            &store,
            EXECUTION,
            &recorder,
            "Gather",
        )
        .expect_err("the missing anchor must fail");
        assert!(matches!(error, Error::Lua(_)));

        let observations = recorder.observations();
        assert_eq!(
            observations,
            vec![
                ("Gather".to_string(), detail::STORE_WRITE_SUCCEEDED.clone()),
                (
                    "Gather".to_string(),
                    detail::STORE_READ_LINES_SUCCEEDED.clone(),
                ),
                ("Gather".to_string(), detail::STORE_REPLACE_FAILED.clone()),
            ]
        );
        let trace = format!("{observations:?}");
        for payload in [
            "secret/path.txt",
            "private contents",
            "missing secret",
            "replacement",
            "private input",
        ] {
            assert!(
                !trace.contains(payload),
                "observation leaked payload {payload:?}: {trace}"
            );
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "parametric coverage of all store ops"
    )]
    fn every_store_operation_reports_its_exact_success_and_failure() {
        struct Case {
            source: &'static str,
            success: Observation,
            failure: Observation,
            prepare: fn(&StoreRef),
        }

        fn empty(_store: &StoreRef) {}

        fn existing(store: &StoreRef) {
            store
                .write("a.txt", "old")
                .expect("the memory store can prepare a file");
        }

        let cases = [
            Case {
                source: "store.write('a.txt', 'new')",
                success: detail::STORE_WRITE_SUCCEEDED,
                failure: detail::STORE_WRITE_FAILED,
                prepare: empty,
            },
            Case {
                source: "store.append('a.txt', 'new')",
                success: detail::STORE_APPEND_SUCCEEDED,
                failure: detail::STORE_APPEND_FAILED,
                prepare: empty,
            },
            Case {
                source: "store.read_lines('a.txt')",
                success: detail::STORE_READ_LINES_SUCCEEDED,
                failure: detail::STORE_READ_LINES_FAILED,
                prepare: existing,
            },
            Case {
                source: "store.read('a.txt')",
                success: detail::STORE_READ_SUCCEEDED,
                failure: detail::STORE_READ_FAILED,
                prepare: existing,
            },
            Case {
                source: "store.inject('a.txt')",
                success: detail::STORE_INJECT_SUCCEEDED,
                failure: detail::STORE_INJECT_FAILED,
                prepare: existing,
            },
            Case {
                source: "store.str_replace('a.txt', 'old', 'new')",
                success: detail::STORE_REPLACE_SUCCEEDED,
                failure: detail::STORE_REPLACE_FAILED,
                prepare: existing,
            },
            Case {
                source: "store.delete('a.txt')",
                success: detail::STORE_DELETE_SUCCEEDED,
                failure: detail::STORE_DELETE_FAILED,
                prepare: existing,
            },
            Case {
                source: "local matches = store.glob('*.txt')",
                success: detail::STORE_GLOB_SUCCEEDED,
                failure: detail::STORE_GLOB_FAILED,
                prepare: existing,
            },
        ];

        for case in cases {
            let store = StoreRef::memory();
            (case.prepare)(&store);
            let recorder = Recorder::default();
            run_chunk(
                case.source,
                "",
                &json!({}),
                &store,
                EXECUTION,
                &recorder,
                "StoreRef",
            )
            .expect("the memory store operation succeeds");
            assert_eq!(
                recorder.observations(),
                vec![("StoreRef".to_owned(), case.success.clone())],
                "wrong success observation for {}",
                case.source
            );

            let store = StoreRef::new(Box::new(FailingStore));
            let recorder = Recorder::default();
            let error = run_chunk(
                case.source,
                "",
                &json!({}),
                &store,
                EXECUTION,
                &recorder,
                "StoreRef",
            )
            .expect_err("the failing backend rejects every operation");
            assert!(matches!(error, Error::Lua(_)));
            assert_eq!(
                recorder.observations(),
                vec![("StoreRef".to_owned(), case.failure.clone())],
                "wrong failure observation for {}",
                case.source
            );
        }
    }

    #[test]
    fn store_observations_happen_before_later_lua_side_effects() {
        let store = StoreRef::memory();
        let recorder = BoundaryRecorder {
            store: store.clone(),
            snapshots: Mutex::new(Vec::new()),
        };

        run_chunk(
            "store.write('first.txt', '')\nstore.write('second.txt', '')",
            "",
            &json!({}),
            &store,
            EXECUTION,
            &recorder,
            "StoreRef",
        )
        .expect("both writes succeed");

        assert_eq!(
            *recorder
                .snapshots
                .lock()
                .expect("the snapshot mutex must not be poisoned"),
            vec![
                vec!["first.txt".to_owned()],
                vec!["first.txt".to_owned(), "second.txt".to_owned()],
            ]
        );
    }
}
