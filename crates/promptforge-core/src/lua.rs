//! Sandboxed Lua execution for a section's Lua block.
//!
//! A section's Lua chunk runs in a fresh, restricted `mlua` VM: only the
//! `string`, `table`, and `math` standard libraries plus the safe base
//! functions are available; the raw input `args` string and the runtime `sys`
//! table are exposed; a writable `var` table is provided for the block to
//! populate; an always-on `store` table gives the block the run's virtual
//! files; and an instruction-count hook aborts a runaway block.
//!
//! The chunk's top-level return value becomes the section's result (the finish
//! case of the exit rule). The `var` table is read back afterward as JSON for
//! prose substitution.
//!
//! The `store` table is a deterministic host capability (like `var`), always
//! present and independent of tool scoping. Its methods are backed by the
//! run-scoped [`Store`] handle threaded in from the executor, so every section
//! in a run shares one set of virtual files even though contexts clear on each
//! transition. A failed store op raises a Lua error, which surfaces from
//! [`run_chunk`] as [`Error::Lua`].

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use mlua::{
    Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, MultiValue, Scope, StdLib, Value,
    Variadic, VmState,
};
use serde_json::Value as Json;

use crate::observe::{Observer, detail};
use crate::store::Store;
use crate::tools::ToolId;
use crate::{Error, Result};

/// How many instructions between hook firings.
const HOOK_INTERVAL: u32 = 10_000;
/// Maximum number of hook firings before a block is aborted (~1e7 instructions).
const HOOK_BUDGET: u64 = 1_000;

/// Resolves one plain-English capability description to one stable live tool.
///
/// This is the deterministic seam used by Lua declaration binding. It keeps
/// core independent of any concrete picker implementation while allowing a
/// caller to supply a fixed resolver in tests or adapt a picker later.
pub trait ToolResolver: Send + Sync {
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
pub struct ToolBinding {
    alias: String,
    description: String,
    id: ToolId,
}

impl ToolBinding {
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
}

/// Immutable prompt-level tool bindings produced by one H1 declaration pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolBindings {
    bindings: Vec<ToolBinding>,
    always: Vec<String>,
    declarations: Vec<ToolDeclaration>,
}

impl ToolBindings {
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

    fn binding(&self, alias: &str) -> Option<&ToolBinding> {
        self.bindings.iter().find(|binding| binding.alias == alias)
    }
}

/// A closed H2 tool scope, ordered with prompt-wide aliases first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolScope {
    bindings: Vec<ToolBinding>,
}

impl ToolScope {
    /// Returns the effective bindings in model-advertisement order.
    #[must_use]
    pub fn bindings(&self) -> &[ToolBinding] {
        &self.bindings
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolPhase {
    Replay,
    H2,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolDeclaration {
    Need { alias: String, description: String },
    Always(String),
}

#[derive(Debug)]
struct ToolRuntime {
    phase: ToolPhase,
    declaration_index: usize,
    added: Vec<String>,
}

/// Compiled Lua 5.4 source that can be loaded into multiple process-local VMs.
///
/// A program retains its original source for diagnostics and stores bytecode
/// produced once by Lua 5.4. The bytecode is an in-memory implementation detail:
/// it is not a stable or portable serialization format and must not be persisted.
///
/// Compilation does not execute the source. Loading a program with [`load`](Self::load)
/// creates a function in the supplied VM but likewise does not call it.
#[derive(Debug, Clone)]
pub struct LuaProgram {
    source: String,
    bytecode: Vec<u8>,
}

impl LuaProgram {
    /// Compiles `source` as Lua 5.4 bytecode without executing it.
    ///
    /// `location` identifies the source region in diagnostics. Compilation
    /// reports contain only fixed strings and never include `source` or
    /// `location`.
    ///
    /// # Errors
    /// Returns [`Error::LuaCompile`] when `source` is not syntactically valid,
    /// retaining the source, location, and Lua diagnostic. Returns
    /// [`Error::Lua`] if the temporary compiler VM cannot be created.
    ///
    /// # Examples
    /// ```
    /// use mlua::Lua;
    /// use promptforge_core::lua::LuaProgram;
    /// use promptforge_core::observe::NullObserver;
    ///
    /// let program = LuaProgram::compile(
    ///     "return 40 + 2",
    ///     "example preamble",
    ///     &NullObserver,
    ///     "Example",
    /// )?;
    /// let lua = Lua::new();
    /// let chunk = program.load(&lua)?;
    /// let answer: i64 = chunk.call(())?;
    /// assert_eq!(answer, 42);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn compile(
        source: &str,
        location: &str,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Self> {
        observer.observe(section, detail::LUA_COMPILATION_STARTED);

        let lua = match Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH,
            LuaOptions::default(),
        ) {
            Ok(lua) => lua,
            Err(error) => {
                observer.observe(section, detail::LUA_COMPILATION_FAILED);
                return Err(Error::Lua(error.to_string()));
            }
        };

        let function = match lua.load(source).set_name(location).into_function() {
            Ok(function) => function,
            Err(error) => {
                observer.observe(section, detail::LUA_COMPILATION_FAILED);
                return Err(Error::LuaCompile {
                    location: location.to_owned(),
                    lua_source: source.to_owned(),
                    message: error.to_string(),
                });
            }
        };
        let bytecode = function.dump(true);

        observer.observe(section, detail::LUA_COMPILATION_SUCCEEDED);
        Ok(Self {
            source: source.to_owned(),
            bytecode,
        })
    }

    /// Returns the original Lua source retained for diagnostics.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Loads the compiled function into `lua` without executing it.
    ///
    /// The bytecode is loaded only into a VM in the same process and is never
    /// exposed as a persistence format.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the VM rejects the internally compiled
    /// bytecode.
    pub fn load(&self, lua: &Lua) -> Result<Function> {
        lua.load(self.bytecode.as_slice())
            .into_function()
            .map_err(|error| Error::Lua(error.to_string()))
    }
}

#[derive(Debug, Default)]
struct BindingState {
    bindings: Vec<ToolBinding>,
    always: Vec<String>,
    declarations: Vec<ToolDeclaration>,
}

/// Executes an H1 shared program in binding mode and freezes its declarations.
///
/// In binding mode `tools.need(alias, description)` resolves each capability
/// exactly once and `tools.always(alias)` marks a previously declared alias as
/// prompt-wide. `tools.add` is unavailable. Aliases are case-sensitive ASCII
/// identifiers matching `[A-Za-z][A-Za-z0-9_-]{0,63}`.
///
/// # Errors
/// Returns [`Error::Lua`] for invalid aliases, duplicate declarations,
/// out-of-order or duplicate `tools.always` calls, resolver failures, Lua
/// failures, or a non-nil top-level return.
///
/// # Examples
/// ```
/// use promptforge_core::lua::{LuaProgram, bind_tool_declarations};
/// use promptforge_core::observe::NullObserver;
/// use promptforge_core::tools::ToolId;
///
/// let shared = LuaProgram::compile(
///     "tools.need('Web-Search', 'search the web'); tools.always('Web-Search')",
///     "shared",
///     &NullObserver,
///     "Example",
/// )?;
/// let resolver = |_: &str| Ok(ToolId::new("example", "search"));
/// let bindings =
///     bind_tool_declarations(&shared, &resolver, &NullObserver, "Example")?;
/// assert_eq!(bindings.bindings()[0].alias(), "Web-Search");
/// assert_eq!(bindings.always(), ["Web-Search"]);
/// # Ok::<(), promptforge_core::Error>(())
/// ```
pub fn bind_tool_declarations(
    program: &LuaProgram,
    resolver: &dyn ToolResolver,
    observer: &dyn Observer,
    section: &str,
) -> Result<ToolBindings> {
    observer.observe(section, detail::TOOL_BINDING_STARTED);
    let result = bind_tool_declarations_inner(program, resolver);
    observer.observe(
        section,
        if result.is_ok() {
            detail::TOOL_BINDING_SUCCEEDED
        } else {
            detail::TOOL_BINDING_FAILED
        },
    );
    result
}

#[expect(
    clippy::too_many_lines,
    reason = "one scoped Lua table installs the three declaration-mode operations and freezes their shared state"
)]
fn bind_tool_declarations_inner(
    program: &LuaProgram,
    resolver: &dyn ToolResolver,
) -> Result<ToolBindings> {
    let lua = Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH,
        LuaOptions::default(),
    )
    .map_err(|error| Error::Lua(error.to_string()))?;
    harden(&lua)?;
    install_instruction_budget(&lua);
    let state = Arc::new(Mutex::new(BindingState::default()));

    let returned: MultiValue = lua
        .scope(|scope| {
            let tools = lua.create_table()?;

            let needs = Arc::clone(&state);
            let need = scope.create_function(
                move |_, (alias, description): (String, String)| -> mlua::Result<()> {
                    validate_alias(&alias)
                        .map_err(|error| mlua::Error::external(error.to_string()))?;
                    let mut declarations = needs
                        .lock()
                        .map_err(|_| mlua::Error::external("tool binding recorder was poisoned"))?;
                    if declarations
                        .bindings
                        .iter()
                        .any(|binding| binding.alias == alias)
                    {
                        return Err(mlua::Error::external(format!(
                            "tool alias {alias:?} was declared more than once"
                        )));
                    }
                    let id = resolver
                        .resolve(&description)
                        .map_err(|error| mlua::Error::external(error.to_string()))?;
                    declarations.bindings.push(ToolBinding {
                        alias: alias.clone(),
                        description: description.clone(),
                        id,
                    });
                    declarations
                        .declarations
                        .push(ToolDeclaration::Need { alias, description });
                    Ok(())
                },
            )?;
            tools.set("need", need)?;

            let prompt_wide = Arc::clone(&state);
            let always = scope.create_function(move |_, alias: String| -> mlua::Result<()> {
                validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
                let mut declarations = prompt_wide
                    .lock()
                    .map_err(|_| mlua::Error::external("tool binding recorder was poisoned"))?;
                if !declarations
                    .bindings
                    .iter()
                    .any(|binding| binding.alias == alias)
                {
                    return Err(mlua::Error::external(format!(
                        "tools.always alias {alias:?} was not declared by tools.need"
                    )));
                }
                if declarations
                    .always
                    .iter()
                    .any(|existing| existing == &alias)
                {
                    return Err(mlua::Error::external(format!(
                        "tools.always alias {alias:?} was recorded more than once"
                    )));
                }
                declarations.always.push(alias.clone());
                declarations
                    .declarations
                    .push(ToolDeclaration::Always(alias));
                Ok(())
            })?;
            tools.set("always", always)?;
            let add = scope.create_function(|_, _: Variadic<String>| -> mlua::Result<()> {
                Err(mlua::Error::external(
                    "tools.add is only available during H2 recording",
                ))
            })?;
            tools.set("add", add)?;
            lua.globals().raw_set("tools", tools)?;
            program
                .load(&lua)
                .map_err(|error| mlua::Error::external(error.to_string()))?
                .call(())
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    if scalar_return(returned)?.is_some() {
        return Err(Error::Lua(
            "H1 tool declaration program must not return a value".to_owned(),
        ));
    }
    let state = Arc::try_unwrap(state)
        .map_err(|_| Error::Lua("tool binding recorder remained shared".to_owned()))?
        .into_inner()
        .map_err(|_| Error::Lua("tool binding recorder was poisoned".to_owned()))?;
    Ok(ToolBindings {
        bindings: state.bindings,
        always: state.always,
        declarations: state.declarations,
    })
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
/// shared program runs before host values are installed, then preamble and
/// epilog programs loaded with [`run_preamble`](Self::run_preamble) and
/// [`run_epilog`](Self::run_epilog) see that same environment.
/// [`bind_reply`](Self::bind_reply) inserts the model reply into it between
/// those phases. A single instruction counter covers every program run by this
/// VM, so splitting work across lifecycle phases cannot reset the budget.
///
/// `SectionVm` deliberately does not expose its underlying [`Lua`]. This keeps
/// hardening, host injection, instruction accounting, and report delivery on
/// the one owned path. Each section must receive a new instance; dropping it
/// destroys all Lua memory belonging to that section.
///
/// # Examples
/// ```
/// use promptforge_core::lua::SectionVm;
/// use promptforge_core::observe::NullObserver;
///
/// let vm = SectionVm::new(None, &NullObserver, "Example")?;
/// vm.teardown(&NullObserver, "Example");
/// # Ok::<(), promptforge_core::Error>(())
/// ```
#[derive(Debug)]
pub struct SectionVm {
    lua: Lua,
    scoped_tools: Arc<Mutex<Vec<String>>>,
    bound_tools: Option<ToolBindings>,
    tool_runtime: Option<Arc<Mutex<ToolRuntime>>>,
    store: Option<Store>,
    host_injected: bool,
}

impl SectionVm {
    /// Creates a hardened section VM and optionally executes a shared program.
    ///
    /// The shared program runs before `args`, `sys`, `var`, `tools`, `store`,
    /// and `reply` are installed. This delayed injection prevents shared code
    /// from retaining a host value before section execution begins.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the VM cannot be built or hardened, or if the
    /// shared program fails or returns a non-scalar value.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::lua::{LuaProgram, SectionVm};
    /// use promptforge_core::observe::NullObserver;
    ///
    /// let shared = LuaProgram::compile(
    ///     "function decorate(s) return '<' .. s .. '>' end",
    ///     "shared",
    ///     &NullObserver,
    ///     "Example",
    /// )?;
    /// let vm = SectionVm::new(Some(&shared), &NullObserver, "Example")?;
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub fn new(
        shared: Option<&LuaProgram>,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Self> {
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH,
            LuaOptions::default(),
        )
        .map_err(|error| Error::Lua(error.to_string()))?;
        harden(&lua)?;
        install_instruction_budget(&lua);

        let vm = Self {
            lua,
            scoped_tools: Arc::new(Mutex::new(Vec::new())),
            bound_tools: None,
            tool_runtime: None,
            store: None,
            host_injected: false,
        };
        if let Some(program) = shared {
            observer.observe(section, detail::LUA_SHARED_LOAD_STARTED);
            match vm.run_loaded(program) {
                Ok(_) => observer.observe(section, detail::LUA_SHARED_LOAD_SUCCEEDED),
                Err(error) => {
                    observer.observe(section, detail::LUA_SHARED_LOAD_FAILED);
                    return Err(error);
                }
            }
        }
        Ok(vm)
    }

    /// Creates a section VM and replays frozen H1 tool declarations exactly.
    ///
    /// The shared program runs again so its library definitions populate this
    /// section's isolated environment. During that run, `tools.need` and
    /// `tools.always` must reproduce the binding pass call-for-call. No resolver
    /// is consulted. A changed alias, description, order, omitted call, or extra
    /// call is a replay mismatch.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the VM cannot be built, shared execution fails,
    /// a declaration differs from `bindings`, or shared code returns a value.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::lua::{
    ///     LuaProgram, SectionVm, bind_tool_declarations,
    /// };
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::tools::ToolId;
    ///
    /// let shared = LuaProgram::compile(
    ///     "tools.need('search', 'search the web')",
    ///     "shared",
    ///     &NullObserver,
    ///     "Example",
    /// )?;
    /// let resolver = |_: &str| Ok(ToolId::new("example", "search"));
    /// let bindings =
    ///     bind_tool_declarations(&shared, &resolver, &NullObserver, "Example")?;
    /// let vm =
    ///     SectionVm::new_with_bindings(&shared, &bindings, &NullObserver, "Example")?;
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub fn new_with_bindings(
        shared: &LuaProgram,
        bindings: &ToolBindings,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Self> {
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH,
            LuaOptions::default(),
        )
        .map_err(|error| Error::Lua(error.to_string()))?;
        harden(&lua)?;
        install_instruction_budget(&lua);
        let runtime = Arc::new(Mutex::new(ToolRuntime {
            phase: ToolPhase::Replay,
            declaration_index: 0,
            added: Vec::new(),
        }));
        install_replay_tools(&lua, bindings, &runtime)?;

        let vm = Self {
            lua,
            scoped_tools: Arc::new(Mutex::new(Vec::new())),
            bound_tools: Some(bindings.clone()),
            tool_runtime: Some(runtime),
            store: None,
            host_injected: false,
        };
        observer.observe(section, detail::LUA_SHARED_LOAD_STARTED);
        observer.observe(section, detail::TOOL_REPLAY_STARTED);
        let result = vm.run_loaded(shared).and_then(|returned| {
            if returned.is_some() {
                Err(Error::Lua(
                    "H1 tool declaration program must not return a value".to_owned(),
                ))
            } else {
                finish_replay(&vm)
            }
        });
        observer.observe(
            section,
            if result.is_ok() {
                detail::TOOL_REPLAY_SUCCEEDED
            } else {
                detail::TOOL_REPLAY_FAILED
            },
        );
        observer.observe(
            section,
            if result.is_ok() {
                detail::LUA_SHARED_LOAD_SUCCEEDED
            } else {
                detail::LUA_SHARED_LOAD_FAILED
            },
        );
        result.map(|()| vm)
    }

    /// Installs the section's host values after the shared program has run.
    ///
    /// This operation may be called exactly once. The store callbacks own a
    /// clone of the run-scoped store. Store functions are installed with
    /// phase-local borrowed observation context by
    /// [`run_preamble`](Self::run_preamble) and [`run_epilog`](Self::run_epilog),
    /// so no observer reference is retained while the VM waits for a model reply.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values cannot be bridged or if host
    /// values were already injected.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::lua::SectionVm;
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::Store;
    ///
    /// let mut vm = SectionVm::new(None, &NullObserver, "Example")?;
    /// vm.inject_host("input", &serde_json::json!({ "id": 1 }), &Store::memory())?;
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub fn inject_host(&mut self, args: &str, sys: &Json, store: &Store) -> Result<()> {
        if self.host_injected {
            return Err(Error::Lua(
                "section VM host values were already injected".to_owned(),
            ));
        }

        let globals = self.lua.globals();
        globals
            .raw_set("args", args)
            .map_err(|error| Error::Lua(error.to_string()))?;
        let sys_value = self
            .lua
            .to_value(sys)
            .map_err(|error| Error::Lua(error.to_string()))?;
        globals
            .raw_set("sys", sys_value)
            .map_err(|error| Error::Lua(error.to_string()))?;
        let var = self
            .lua
            .create_table()
            .map_err(|error| Error::Lua(error.to_string()))?;
        globals
            .raw_set("var", var)
            .map_err(|error| Error::Lua(error.to_string()))?;
        if let (Some(bindings), Some(runtime)) = (&self.bound_tools, &self.tool_runtime) {
            install_h2_tools(&self.lua, &globals, bindings, runtime)?;
        } else {
            install_tools_table(&self.lua, &globals, &self.scoped_tools)?;
        }
        globals
            .raw_set("reply", Value::Nil)
            .map_err(|error| Error::Lua(error.to_string()))?;
        self.store = Some(store.clone());
        self.host_injected = true;
        Ok(())
    }

    /// Executes a compiled preamble in this VM's persistent environment.
    ///
    /// Store-operation reports recorded by host callbacks are delivered in
    /// operation order before this method returns, including when execution
    /// fails. A nil or absent top-level return produces `None`; strings,
    /// integers, numbers, and booleans produce their scalar string form.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected, execution
    /// fails, the shared instruction budget is exhausted, or the program
    /// returns a non-scalar value.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::lua::{LuaProgram, SectionVm};
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::Store;
    ///
    /// let preamble = LuaProgram::compile(
    ///     "var.answer = 42",
    ///     "preamble",
    ///     &NullObserver,
    ///     "Example",
    /// )?;
    /// let mut vm = SectionVm::new(None, &NullObserver, "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &Store::memory())?;
    /// assert_eq!(vm.run_preamble(&preamble, &NullObserver, "Example")?, None);
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub fn run_preamble(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        observer.observe(section, detail::LUA_PREAMBLE_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(section, detail::LUA_PREAMBLE_FAILED);
            return Err(error);
        }
        let result = self.run_loaded_with_host(program, observer, section);
        observer.observe(
            section,
            if result.is_ok() {
                detail::LUA_PREAMBLE_SUCCEEDED
            } else {
                detail::LUA_PREAMBLE_FAILED
            },
        );
        result
    }

    /// Binds the model reply for a later epilog in the same environment.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected, a bound
    /// VM's tool scope remains open, or the reply cannot be installed.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::lua::SectionVm;
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::Store;
    ///
    /// let mut vm = SectionVm::new(None, &NullObserver, "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &Store::memory())?;
    /// vm.bind_reply("model answer", &NullObserver, "Example")?;
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub fn bind_reply(&self, reply: &str, observer: &dyn Observer, section: &str) -> Result<()> {
        observer.observe(section, detail::LUA_REPLY_BINDING_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(section, detail::LUA_REPLY_BINDING_FAILED);
            return Err(error);
        }
        if let Err(error) = self.require_closed_tool_scope("bind a model reply") {
            observer.observe(section, detail::LUA_REPLY_BINDING_FAILED);
            return Err(error);
        }
        let result = self
            .lua
            .globals()
            .raw_set("reply", reply)
            .map_err(|error| Error::Lua(error.to_string()));
        observer.observe(
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
    /// Store-operation reports are delivered in operation order between the
    /// epilog's start and outcome reports.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected, a bound
    /// VM's tool scope remains open, execution fails, the shared instruction
    /// budget is exhausted, or the program returns a non-scalar value.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::lua::{LuaProgram, SectionVm};
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::Store;
    ///
    /// let epilog = LuaProgram::compile(
    ///     "return reply",
    ///     "epilog",
    ///     &NullObserver,
    ///     "Example",
    /// )?;
    /// let mut vm = SectionVm::new(None, &NullObserver, "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &Store::memory())?;
    /// vm.bind_reply("done", &NullObserver, "Example")?;
    /// assert_eq!(
    ///     vm.run_epilog(&epilog, &NullObserver, "Example")?.as_deref(),
    ///     Some("done"),
    /// );
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub fn run_epilog(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        observer.observe(section, detail::LUA_EPILOG_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(section, detail::LUA_EPILOG_FAILED);
            return Err(error);
        }
        if let Err(error) = self.require_closed_tool_scope("run an epilog") {
            observer.observe(section, detail::LUA_EPILOG_FAILED);
            return Err(error);
        }
        let result = self.run_loaded_with_host(program, observer, section);
        observer.observe(
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
    /// ```
    /// use promptforge_core::lua::SectionVm;
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::Store;
    ///
    /// let mut vm = SectionVm::new(None, &NullObserver, "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &Store::memory())?;
    /// assert_eq!(vm.var()?, serde_json::json!({}));
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub fn var(&self) -> Result<Json> {
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

    /// Returns tool names recorded by `tools.add`, in first-seen order.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the internal recorder mutex was poisoned.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::lua::{LuaProgram, SectionVm};
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::Store;
    ///
    /// let preamble = LuaProgram::compile(
    ///     "tools.add('search')",
    ///     "preamble",
    ///     &NullObserver,
    ///     "Example",
    /// )?;
    /// let mut vm = SectionVm::new(None, &NullObserver, "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &Store::memory())?;
    /// vm.run_preamble(&preamble, &NullObserver, "Example")?;
    /// assert_eq!(vm.scoped_tools()?, vec!["search"]);
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub fn scoped_tools(&self) -> Result<Vec<String>> {
        self.scoped_tools
            .lock()
            .map(|names| names.clone())
            .map_err(|_| Error::Lua("section VM tool recorder was poisoned".to_owned()))
    }

    /// Closes and returns this section's effective tool scope.
    ///
    /// Prompt-wide `tools.always` aliases come first, followed by first-seen
    /// `tools.add` aliases from the H2 preamble. Closing is one-way: retained
    /// function references cannot add tools during an epilog.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] for an unbound VM, a poisoned recorder, or a
    /// second closure attempt.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::lua::{
    ///     LuaProgram, SectionVm, bind_tool_declarations,
    /// };
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::Store;
    /// use promptforge_core::tools::ToolId;
    ///
    /// let shared = LuaProgram::compile(
    ///     "tools.need('search', 'search the web')",
    ///     "shared",
    ///     &NullObserver,
    ///     "Example",
    /// )?;
    /// let resolver = |_: &str| Ok(ToolId::new("example", "search"));
    /// let bindings =
    ///     bind_tool_declarations(&shared, &resolver, &NullObserver, "Example")?;
    /// let mut vm =
    ///     SectionVm::new_with_bindings(&shared, &bindings, &NullObserver, "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &Store::memory())?;
    /// let preamble = LuaProgram::compile(
    ///     "tools.add('search')",
    ///     "preamble",
    ///     &NullObserver,
    ///     "Example",
    /// )?;
    /// vm.run_preamble(&preamble, &NullObserver, "Example")?;
    /// let scope = vm.close_tool_scope(&NullObserver, "Example")?;
    /// assert_eq!(scope.bindings()[0].alias(), "search");
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub fn close_tool_scope(&self, observer: &dyn Observer, section: &str) -> Result<ToolScope> {
        observer.observe(section, detail::TOOL_SCOPE_CLOSING);
        let result = self.close_tool_scope_inner();
        observer.observe(
            section,
            if result.is_ok() {
                detail::TOOL_SCOPE_CLOSED
            } else {
                detail::TOOL_SCOPE_FAILED
            },
        );
        result
    }

    fn close_tool_scope_inner(&self) -> Result<ToolScope> {
        let bindings = self
            .bound_tools
            .as_ref()
            .ok_or_else(|| Error::Lua("section VM has no prompt-level tool bindings".to_owned()))?;
        let runtime = self
            .tool_runtime
            .as_ref()
            .ok_or_else(|| Error::Lua("section VM has no tool declaration runtime".to_owned()))?;
        let mut runtime = runtime
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
            .map(|alias| {
                bindings.binding(alias).cloned().ok_or_else(|| {
                    Error::Lua(format!("tool alias {alias:?} has no frozen binding"))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ToolScope {
            bindings: effective,
        })
    }

    fn require_closed_tool_scope(&self, operation: &str) -> Result<()> {
        let Some(runtime) = &self.tool_runtime else {
            return Ok(());
        };
        let runtime = runtime
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
    /// ```
    /// use promptforge_core::lua::SectionVm;
    /// use promptforge_core::observe::NullObserver;
    ///
    /// let vm = SectionVm::new(None, &NullObserver, "Example")?;
    /// vm.teardown(&NullObserver, "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub fn teardown(self, observer: &dyn Observer, section: &str) {
        observer.observe(section, detail::LUA_TEARDOWN_STARTED);
        drop(self);
        observer.observe(section, detail::LUA_TEARDOWN_SUCCEEDED);
    }

    fn run_loaded(&self, program: &LuaProgram) -> Result<Option<String>> {
        let returned: MultiValue = program
            .load(&self.lua)?
            .call(())
            .map_err(|error| Error::Lua(error.to_string()))?;
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
                install_store_table(
                    &self.lua,
                    scope,
                    &self.lua.globals(),
                    store,
                    observer,
                    section,
                )
                .map_err(|error| mlua::Error::external(error.to_string()))?;
                program
                    .load(&self.lua)
                    .map_err(|error| mlua::Error::external(error.to_string()))?
                    .call(())
            })
            .map_err(|error| Error::Lua(error.to_string()))?;
        scalar_return(returned)
    }

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
                install_store_table(
                    &self.lua,
                    scope,
                    &self.lua.globals(),
                    store,
                    observer,
                    section,
                )
                .map_err(|error| mlua::Error::external(error.to_string()))?;
                self.lua.load(source).eval()
            })
            .map_err(|error| Error::Lua(error.to_string()))?;
        scalar_return(returned)
    }
}

/// The result of running a section's Lua block.
#[derive(Debug, Clone)]
pub struct LuaOutcome {
    /// The chunk's top-level return value, if it returned one (the finish case).
    pub returned: Option<String>,
    /// The `var` table after the block ran, as JSON, for prose substitution.
    pub var: Json,
    /// The tool names the block advertised with `tools.add(...)`, in first-seen
    /// order and de-duplicated. Empty when the block never called `tools.add`.
    /// These names are recorded verbatim and are not validated here against any
    /// tool registry; the executor resolves them per section.
    pub scoped_tools: Vec<String>,
}

/// Run a section's Lua chunk with `args` and `sys` exposed, a writable `var`
/// table available, and a `store` table backed by `store`, returning the
/// chunk's return value and the final `var`. Harness-mediated store operations
/// report safe outcomes to `observer` under `section`.
///
/// `store` is the run-scoped virtual-file handle; every section in a run is
/// given the same handle, so files a section writes persist for later sections
/// even though each section starts a fresh context. The exposed `store` table
/// is always present (a host capability, not a scoped tool).
///
/// # Errors
/// Returns [`Error::Lua`] if the sandbox cannot be built, `sys`/`var`/`store`
/// cannot be bridged, the chunk fails to run (including hitting the instruction
/// budget or a failing `store` op, which raises a Lua error), or it returns a
/// value that cannot be rendered as a result string.
pub fn run_chunk(
    source: &str,
    args: &str,
    sys: &Json,
    store: &Store,
    observer: &dyn Observer,
    section: &str,
) -> Result<LuaOutcome> {
    let mut vm = SectionVm::new(None, observer, section)?;
    vm.inject_host(args, sys, store)?;
    let returned = vm.run_source(source, observer, section)?;
    let var = vm.var()?;
    let scoped_tools = vm.scoped_tools()?;

    Ok(LuaOutcome {
        returned,
        var,
        scoped_tools,
    })
}

fn install_replay_tools(
    lua: &Lua,
    bindings: &ToolBindings,
    runtime: &Arc<Mutex<ToolRuntime>>,
) -> Result<()> {
    let tools = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;

    let expected = bindings.clone();
    let state = Arc::clone(runtime);
    let need = lua
        .create_function(move |_, (alias, description): (String, String)| {
            validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("tool declaration runtime was poisoned"))?;
            if state.phase != ToolPhase::Replay {
                return Err(mlua::Error::external(
                    "tools.need is only available during H1 binding or replay",
                ));
            }
            let Some(declaration) = expected.declarations.get(state.declaration_index) else {
                return Err(mlua::Error::external(
                    "tool declaration replay had an extra tools.need call",
                ));
            };
            if declaration != &(ToolDeclaration::Need { alias, description }) {
                return Err(mlua::Error::external(format!(
                    "tool declaration replay mismatch at declaration {}",
                    state.declaration_index + 1
                )));
            }
            state.declaration_index += 1;
            Ok(())
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    tools
        .set("need", need)
        .map_err(|error| Error::Lua(error.to_string()))?;

    let expected = bindings.clone();
    let state = Arc::clone(runtime);
    let always = lua
        .create_function(move |_, alias: String| {
            validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("tool declaration runtime was poisoned"))?;
            if state.phase != ToolPhase::Replay {
                return Err(mlua::Error::external(
                    "tools.always is only available during H1 binding or replay",
                ));
            }
            let Some(declaration) = expected.declarations.get(state.declaration_index) else {
                return Err(mlua::Error::external(
                    "tool declaration replay had an extra tools.always call",
                ));
            };
            if declaration != &ToolDeclaration::Always(alias) {
                return Err(mlua::Error::external(format!(
                    "tool declaration replay mismatch at declaration {}",
                    state.declaration_index + 1
                )));
            }
            state.declaration_index += 1;
            Ok(())
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    tools
        .set("always", always)
        .map_err(|error| Error::Lua(error.to_string()))?;

    let add = lua
        .create_function(|_, _: Variadic<String>| -> mlua::Result<()> {
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

fn finish_replay(vm: &SectionVm) -> Result<()> {
    let bindings = vm
        .bound_tools
        .as_ref()
        .ok_or_else(|| Error::Lua("section VM has no frozen tool bindings".to_owned()))?;
    let runtime = vm
        .tool_runtime
        .as_ref()
        .ok_or_else(|| Error::Lua("section VM has no tool declaration runtime".to_owned()))?;
    let runtime = runtime
        .lock()
        .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
    if runtime.declaration_index != bindings.declarations.len() {
        return Err(Error::Lua(format!(
            "tool declaration replay ended after {}/{} declarations",
            runtime.declaration_index,
            bindings.declarations.len()
        )));
    }
    Ok(())
}

fn install_h2_tools(
    lua: &Lua,
    globals: &mlua::Table,
    bindings: &ToolBindings,
    runtime: &Arc<Mutex<ToolRuntime>>,
) -> Result<()> {
    {
        let mut state = runtime
            .lock()
            .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
        if state.phase != ToolPhase::Replay {
            return Err(Error::Lua(
                "tool declaration runtime did not finish replay".to_owned(),
            ));
        }
        state.phase = ToolPhase::H2;
    }

    let tools = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;
    for name in ["need", "always"] {
        let operation = name;
        let forbidden = lua
            .create_function(move |_, _: MultiValue| -> mlua::Result<()> {
                Err(mlua::Error::external(format!(
                    "tools.{operation} is only available during H1 binding or replay"
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
        .create_function(move |_, aliases: Variadic<String>| {
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("tool declaration runtime was poisoned"))?;
            if state.phase != ToolPhase::H2 {
                return Err(mlua::Error::external(
                    "tools.add is only available before the H2 tool scope closes",
                ));
            }
            for alias in aliases.iter() {
                validate_alias(alias).map_err(|error| mlua::Error::external(error.to_string()))?;
                if frozen.binding(alias).is_none() {
                    return Err(mlua::Error::external(format!(
                        "tools.add alias {alias:?} was not declared by tools.need"
                    )));
                }
            }
            for alias in aliases {
                if frozen.always.iter().any(|existing| existing == &alias) {
                    continue;
                }
                if !state.added.iter().any(|existing| existing == &alias) {
                    state.added.push(alias);
                }
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

/// Expose a `tools` table whose `add` host function records tool names into a
/// shared, ordered, de-duplicated collection, and return a handle to that
/// collection so the caller can read the accumulated names back after the
/// chunk runs. `add` only records names; it validates nothing and never
/// touches the model.
///
/// # Errors
/// Returns [`Error::Lua`] if the `tools` table or its `add` function cannot be
/// created or installed into the sandbox globals.
fn install_tools_table(
    lua: &Lua,
    globals: &mlua::Table,
    scoped: &Arc<Mutex<Vec<String>>>,
) -> Result<()> {
    let recorder = Arc::clone(scoped);
    let add = lua
        .create_function(move |_, names: Variadic<String>| {
            let mut acc = recorder
                .lock()
                .map_err(|_| mlua::Error::external("tool recorder was poisoned"))?;
            for name in names {
                if !acc.iter().any(|existing| existing == &name) {
                    acc.push(name);
                }
            }
            Ok(())
        })
        .map_err(|e| Error::Lua(e.to_string()))?;
    let tools = lua.create_table().map_err(|e| Error::Lua(e.to_string()))?;
    tools
        .set("add", add)
        .map_err(|e| Error::Lua(e.to_string()))?;
    globals
        .raw_set("tools", tools)
        .map_err(|e| Error::Lua(e.to_string()))?;
    Ok(())
}

/// Expose an always-on `store` table whose six methods (`write`, `append`,
/// `read`, `str_replace`, `delete`, `glob`) are backed by the run-scoped
/// [`Store`] handle. The functions borrow the observer within an [`mlua::Scope`]
/// so each operation reports immediately after its result is known.
///
/// The table is a deterministic host capability, present regardless of tool
/// scoping. The mutating ops (`write`/`append`/`str_replace`/`delete`) return
/// nil; `read` returns the file's numbered-line string; `glob` returns an
/// array table of matching paths. A [`StoreError`] from any op is mapped into
/// an `mlua` error via [`mlua::Error::external`], so it aborts the chunk and
/// surfaces from [`run_chunk`] as [`Error::Lua`].
///
/// The `Store` handle locks a mutex internally per call and is synchronous, so
/// nothing is held across an await.
///
/// [`StoreError`]: crate::store::StoreError
///
/// # Errors
/// Returns [`Error::Lua`] if the `store` table or any of its functions cannot
/// be created or installed into the sandbox globals.
fn observe_store_result(
    observer: &dyn Observer,
    section: &str,
    succeeded: bool,
    success: &'static str,
    failure: &'static str,
) {
    observer.observe(section, if succeeded { success } else { failure });
}

#[expect(
    clippy::too_many_lines,
    reason = "one table installs all six store operations beside their matching observation outcomes"
)]
fn install_store_table<'scope, 'env: 'scope>(
    lua: &Lua,
    scope: &'scope Scope<'scope, 'env>,
    globals: &mlua::Table,
    store: &Store,
    observer: &'env dyn Observer,
    section: &'env str,
) -> Result<()> {
    let table = lua.create_table().map_err(|e| Error::Lua(e.to_string()))?;

    let handle = store.clone();
    let write = scope
        .create_function(move |_, (path, contents): (String, String)| {
            let result = handle.write(&path, &contents);
            observe_store_result(
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
    let read = scope
        .create_function(move |_, path: String| {
            let result = handle.read(&path);
            observe_store_result(
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
    let str_replace = scope
        .create_function(move |_, (path, old, new): (String, String, String)| {
            let result = handle.str_replace(&path, &old, &new);
            observe_store_result(
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

    globals
        .raw_set("store", table)
        .map_err(|e| Error::Lua(e.to_string()))?;
    Ok(())
}

/// Remove code-loading and reflection globals the base library provides. The
/// `io`, `os`, `package`, `coroutine`, and `debug` libraries are never loaded.
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
    ] {
        globals
            .set(name, Value::Nil)
            .map_err(|e| Error::Lua(e.to_string()))?;
    }
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
    use std::sync::Mutex;

    use super::*;
    use crate::observe::NullObserver;
    use crate::store::{FileStore, StoreError};
    use serde_json::json;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, String)>>);

    impl Observer for Recorder {
        fn observe(&self, section: &str, detail: &str) {
            self.0
                .lock()
                .expect("the recorder mutex must not be poisoned")
                .push((section.to_owned(), detail.to_owned()));
        }
    }

    impl Recorder {
        fn observations(&self) -> Vec<(String, String)> {
            self.0
                .lock()
                .expect("the recorder mutex must not be poisoned")
                .clone()
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

    impl FileStore for FailingStore {
        fn write(&mut self, path: &str, _contents: &str) -> std::result::Result<(), StoreError> {
            Err(Self::error(path))
        }

        fn append(&mut self, path: &str, _contents: &str) -> std::result::Result<(), StoreError> {
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
    }

    struct BoundaryRecorder {
        store: Store,
        snapshots: Mutex<Vec<Vec<String>>>,
    }

    impl Observer for BoundaryRecorder {
        fn observe(&self, _section: &str, _detail: &str) {
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
            &Store::memory(),
            &NullObserver,
            "Test",
        )
    }

    /// Run a chunk against a caller-supplied store, so a test can inspect the
    /// store after the chunk has run.
    fn run_with(source: &str, store: &Store) -> Result<LuaOutcome> {
        run_chunk(
            source,
            "",
            &json!({ "id": 1, "when": "t" }),
            store,
            &NullObserver,
            "Test",
        )
    }

    fn program(source: &str) -> LuaProgram {
        LuaProgram::compile(source, "test program", &NullObserver, "Test")
            .expect("test Lua must compile")
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
            ))
        };
        let bindings = bind_tool_declarations(&shared, &resolver, &NullObserver, "Prompt")
            .expect("fixture declarations must bind");
        (shared, bindings)
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
    fn binding_validates_aliases_exactly() {
        let resolver = |_: &str| Ok(ToolId::new("fixtures", "search"));

        for alias in [
            "",
            "_leading",
            "has.dot",
            "nonasciié",
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-a",
        ] {
            let declaration = program(&format!("tools.need({alias:?}, 'capability')"));
            let error = bind_tool_declarations(&declaration, &resolver, &NullObserver, "Prompt")
                .expect_err("invalid aliases must be rejected");
            assert!(
                error.to_string().contains("invalid tool alias"),
                "wrong error for {alias:?}: {error}"
            );
        }

        for valid in ["Upper", "has-dash", &format!("A{}", "2".repeat(63))] {
            let declaration = program(&format!("tools.need({valid:?}, 'capability')"));
            bind_tool_declarations(&declaration, &resolver, &NullObserver, "Prompt")
                .expect("planned alias forms must be valid");
        }
    }

    #[test]
    fn binding_rejects_duplicate_aliases() {
        let resolver = |_: &str| Ok(ToolId::new("fixtures", "search"));
        let error = bind_tool_declarations(
            &program("tools.need('search', 'one'); tools.need('search', 'two')"),
            &resolver,
            &NullObserver,
            "Prompt",
        )
        .expect_err("duplicate aliases must fail");
        assert!(error.to_string().contains("more than once"));
    }

    #[test]
    fn binding_rejects_unknown_and_duplicate_always_aliases() {
        let resolver = |_: &str| Ok(ToolId::new("fixtures", "search"));
        for source in [
            "tools.always('missing')",
            "tools.need('search', 'one'); tools.always('search'); tools.always('search')",
        ] {
            let error =
                bind_tool_declarations(&program(source), &resolver, &NullObserver, "Prompt")
                    .expect_err("invalid always declarations must fail");
            assert!(
                error.to_string().contains("not declared")
                    || error.to_string().contains("more than once")
            );
        }
    }

    #[test]
    fn replay_is_exact_and_never_calls_a_resolver() {
        let (_, bindings) =
            fixture_bindings("tools.need('search', 'search the web'); tools.always('search')");
        for source in [
            "tools.need('search', 'changed'); tools.always('search')",
            "tools.always('search'); tools.need('search', 'search the web')",
            "tools.need('search', 'search the web')",
            "tools.need('search', 'search the web'); tools.always('search'); tools.always('search')",
        ] {
            let error =
                SectionVm::new_with_bindings(&program(source), &bindings, &NullObserver, "Section")
                    .expect_err("changed declarations must fail replay");
            assert!(error.to_string().contains("replay"));
        }
    }

    #[test]
    fn h2_recording_closes_to_always_then_added_scope() {
        let (shared, bindings) = fixture_bindings(
            "tools.need('search', 'search the web'); \
             tools.need('fetch', 'fetch a page'); \
             tools.always('search')",
        );
        let preamble = program("tools.add('fetch', 'search', 'fetch')");
        let mut vm = SectionVm::new_with_bindings(&shared, &bindings, &NullObserver, "Section")
            .expect("declarations must replay");
        vm.inject_host("", &json!({}), &Store::memory())
            .expect("host must inject");
        vm.run_preamble(&preamble, &NullObserver, "Section")
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
    fn empty_add_is_a_no_op_and_failed_variadic_add_is_atomic() {
        let (shared, bindings) = fixture_bindings(
            "tools.need('search', 'search the web'); \
             tools.need('fetch', 'fetch a page')",
        );
        let preamble = program(
            "tools.add(); \
             local ok = pcall(tools.add, 'search', 'missing'); \
             if ok then error('invalid add unexpectedly succeeded') end; \
             tools.add('fetch')",
        );
        let mut vm = SectionVm::new_with_bindings(&shared, &bindings, &NullObserver, "Section")
            .expect("declarations must replay");
        vm.inject_host("", &json!({}), &Store::memory())
            .expect("host must inject");
        vm.run_preamble(&preamble, &NullObserver, "Section")
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
        let mut vm = SectionVm::new_with_bindings(&shared, &bindings, &NullObserver, "Section")
            .expect("declarations must replay");
        vm.inject_host("", &json!({}), &Store::memory())
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
        let source = "saved_need = tools.need\n\
                      tools.need('search', 'search the web')";
        let (shared, bindings) = fixture_bindings(source);
        let mut vm = SectionVm::new_with_bindings(&shared, &bindings, &NullObserver, "Section")
            .expect("declarations must replay");
        vm.inject_host("", &json!({}), &Store::memory())
            .expect("host must inject");

        let error = vm
            .run_preamble(
                &program("saved_need('other', 'fetch a page')"),
                &NullObserver,
                "Section",
            )
            .expect_err("captured H1 functions must not run in H2");
        assert!(error.to_string().contains("H1 binding or replay"));

        let error = vm
            .run_preamble(
                &program("tools.need('other', 'fetch a page')"),
                &NullObserver,
                "Section",
            )
            .expect_err("current H2 table must reject need");
        assert!(error.to_string().contains("H1 binding or replay"));
    }

    #[test]
    fn unknown_h2_alias_fails_before_scope_closure() {
        let (shared, bindings) = fixture_bindings("tools.need('search', 'search the web')");
        let mut vm = SectionVm::new_with_bindings(&shared, &bindings, &NullObserver, "Section")
            .expect("declarations must replay");
        vm.inject_host("", &json!({}), &Store::memory())
            .expect("host must inject");
        let error = vm
            .run_preamble(&program("tools.add('missing')"), &NullObserver, "Section")
            .expect_err("only declared aliases may enter H2 scope");
        assert!(error.to_string().contains("not declared"));
    }

    #[test]
    fn declaration_reports_are_exact_ordered_and_payload_free() {
        let resolver = |_: &str| Ok(ToolId::new("fixtures", "search"));
        let shared = program("tools.need('private_alias', 'private capability')");
        let recorder = Recorder::default();
        let bindings = bind_tool_declarations(&shared, &resolver, &recorder, "Prompt")
            .expect("binding must succeed");
        let mut vm = SectionVm::new_with_bindings(&shared, &bindings, &recorder, "Section")
            .expect("replay must succeed");
        vm.inject_host("", &json!({}), &Store::memory())
            .expect("host must inject");
        vm.close_tool_scope(&recorder, "Section")
            .expect("empty H2 scope must close");

        let observations = recorder.observations();
        assert_eq!(
            observations
                .iter()
                .map(|(_, detail)| detail.as_str())
                .collect::<Vec<_>>(),
            [
                detail::TOOL_BINDING_STARTED,
                detail::TOOL_BINDING_SUCCEEDED,
                detail::LUA_SHARED_LOAD_STARTED,
                detail::TOOL_REPLAY_STARTED,
                detail::TOOL_REPLAY_SUCCEEDED,
                detail::LUA_SHARED_LOAD_SUCCEEDED,
                detail::TOOL_SCOPE_CLOSING,
                detail::TOOL_SCOPE_CLOSED,
            ]
        );
        let trace = format!("{observations:?}");
        assert!(!trace.contains("private_alias"));
        assert!(!trace.contains("private capability"));
    }

    #[test]
    fn scope_closure_failure_reports_exact_payload_free_sequence() {
        let recorder = Recorder::default();
        let vm =
            SectionVm::new(None, &recorder, "private section").expect("unbound VM must construct");
        vm.close_tool_scope(&recorder, "private section")
            .expect_err("unbound scope closure must fail");

        assert_eq!(
            recorder.observations(),
            [
                (
                    "private section".to_owned(),
                    detail::TOOL_SCOPE_CLOSING.to_owned(),
                ),
                (
                    "private section".to_owned(),
                    detail::TOOL_SCOPE_FAILED.to_owned(),
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
        let preamble = program(
            "var.from_shared = decorate(args)\n\
             tools.add('search')\n\
             store.write('phase.txt', var.from_shared)",
        );
        let epilog = program(
            "tools.add('fetch', 'search')\n\
             return shared_saw_args == nil and decorate(reply) or 'host leaked early'",
        );
        let store = Store::memory();
        let mut vm =
            SectionVm::new(Some(&shared), &NullObserver, "Test").expect("shared program must run");
        vm.inject_host("input", &json!({ "id": 7 }), &store)
            .expect("host values must inject");

        assert_eq!(
            vm.run_preamble(&preamble, &NullObserver, "Test")
                .expect("preamble must run"),
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
            store.read("phase.txt").expect("shared store must read"),
            "1| <input>"
        );

        vm.bind_reply("model answer", &NullObserver, "Test")
            .expect("reply must bind into the same environment");
        assert_eq!(
            vm.run_epilog(&epilog, &NullObserver, "Test")
                .expect("epilog must run")
                .as_deref(),
            Some("<model answer>")
        );
        assert_eq!(
            vm.scoped_tools().expect("tool recorder must be readable"),
            vec!["search", "fetch"]
        );
    }

    #[test]
    fn section_vm_requires_delayed_single_host_injection() {
        let no_op = program("return args");
        let store = Store::memory();
        let mut vm = SectionVm::new(None, &NullObserver, "Test").expect("VM must build");

        let error = vm
            .run_preamble(&no_op, &NullObserver, "Test")
            .expect_err("programs cannot run before host injection");
        assert!(error.to_string().contains("not been injected"));

        vm.inject_host("first", &json!({}), &store)
            .expect("first injection must succeed");
        let error = vm
            .inject_host("second", &json!({}), &store)
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
        let mut vm =
            SectionVm::new(Some(&shared), &NullObserver, "Test").expect("shared program must run");
        vm.inject_host("private input", &json!({}), &Store::memory())
            .expect("raw host injection must bypass the shared metatable");

        assert_eq!(
            vm.run_preamble(&inspect, &NullObserver, "Test")
                .expect("inspection must run")
                .as_deref(),
            Some("nil,nil,private input")
        );
    }

    #[test]
    fn section_vm_reports_store_operations_in_each_phase() {
        let write = program("store.write('state.txt', args)");
        let read = program("return store.read('state.txt')");
        let recorder = Recorder::default();
        let mut vm = SectionVm::new(None, &NullObserver, "Gather").expect("VM must build");
        vm.inject_host("private input", &json!({}), &Store::memory())
            .expect("host values must inject");

        vm.run_preamble(&write, &recorder, "Gather")
            .expect("preamble write must run");
        vm.bind_reply("private reply", &recorder, "Gather")
            .expect("reply must bind");
        vm.run_epilog(&read, &recorder, "Gather")
            .expect("epilog read must run");
        vm.teardown(&recorder, "Gather");

        assert_eq!(
            recorder.observations(),
            vec![
                ("Gather".to_owned(), detail::LUA_PREAMBLE_STARTED.to_owned(),),
                (
                    "Gather".to_owned(),
                    detail::STORE_WRITE_SUCCEEDED.to_owned(),
                ),
                (
                    "Gather".to_owned(),
                    detail::LUA_PREAMBLE_SUCCEEDED.to_owned(),
                ),
                (
                    "Gather".to_owned(),
                    detail::LUA_REPLY_BINDING_STARTED.to_owned(),
                ),
                (
                    "Gather".to_owned(),
                    detail::LUA_REPLY_BINDING_SUCCEEDED.to_owned(),
                ),
                ("Gather".to_owned(), detail::LUA_EPILOG_STARTED.to_owned(),),
                ("Gather".to_owned(), detail::STORE_READ_SUCCEEDED.to_owned(),),
                ("Gather".to_owned(), detail::LUA_EPILOG_SUCCEEDED.to_owned(),),
                ("Gather".to_owned(), detail::LUA_TEARDOWN_STARTED.to_owned(),),
                (
                    "Gather".to_owned(),
                    detail::LUA_TEARDOWN_SUCCEEDED.to_owned(),
                ),
            ]
        );
        let trace = format!("{:?}", recorder.observations());
        assert!(!trace.contains("private input"));
        assert!(!trace.contains("private reply"));
        assert!(!trace.contains("state.txt"));
    }

    #[test]
    fn section_vm_accepts_only_scalar_top_level_returns() {
        let store = Store::memory();
        for (source, expected) in [
            ("return 'text'", Some("text")),
            ("return 42", Some("42")),
            ("return 1.5", Some("1.5")),
            ("return true", Some("true")),
            ("return nil", None),
        ] {
            let mut vm = SectionVm::new(None, &NullObserver, "Test").expect("VM must build");
            vm.inject_host("", &json!({}), &store)
                .expect("host values must inject");
            assert_eq!(
                vm.run_preamble(&program(source), &NullObserver, "Test")
                    .expect("scalar return must work")
                    .as_deref(),
                expected
            );
        }

        let mut vm = SectionVm::new(None, &NullObserver, "Test").expect("VM must build");
        vm.inject_host("", &json!({}), &store)
            .expect("host values must inject");
        let error = vm
            .run_preamble(&program("return {}"), &NullObserver, "Test")
            .expect_err("table returns must be refused");
        assert!(error.to_string().contains("cannot return a table"));
    }

    #[test]
    fn section_vms_isolate_mutated_shared_globals() {
        let shared = program("counter = 0");
        let increment = program("counter = counter + 1; return counter");
        let store = Store::memory();
        let mut first =
            SectionVm::new(Some(&shared), &NullObserver, "First").expect("first VM must build");
        let mut second =
            SectionVm::new(Some(&shared), &NullObserver, "Second").expect("second VM must build");
        first
            .inject_host("", &json!({}), &store)
            .expect("first host must inject");
        second
            .inject_host("", &json!({}), &store)
            .expect("second host must inject");

        assert_eq!(
            first
                .run_preamble(&increment, &NullObserver, "First")
                .expect("first increment must run")
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            first
                .run_epilog(&increment, &NullObserver, "First")
                .expect("second first-VM increment must run")
                .as_deref(),
            Some("2")
        );
        assert_eq!(
            second
                .run_preamble(&increment, &NullObserver, "Second")
                .expect("second VM increment must run")
                .as_deref(),
            Some("1")
        );
    }

    #[test]
    fn shared_program_consumes_the_later_phase_instruction_budget() {
        let work = program("for i = 1, 3000000 do local value = i end");
        let mut vm = SectionVm::new(Some(&work), &NullObserver, "Test")
            .expect("shared work must fit the budget");
        vm.inject_host("", &json!({}), &Store::memory())
            .expect("host values must inject");

        let error = vm
            .run_preamble(&work, &NullObserver, "Test")
            .expect_err("the preamble must exhaust the budget left by shared execution");
        assert!(error.to_string().contains("instruction budget exceeded"));
    }

    #[test]
    fn section_lifecycle_reports_are_ordered_exact_and_payload_free() {
        let shared = program("private_global = 'shared secret'");
        let preamble = program("var.value = args");
        let epilog = program("return reply");
        let recorder = Recorder::default();
        let mut vm =
            SectionVm::new(Some(&shared), &recorder, "Gather").expect("shared program must run");
        vm.inject_host("private input", &json!({}), &Store::memory())
            .expect("host values must inject");
        vm.run_preamble(&preamble, &recorder, "Gather")
            .expect("preamble must run");
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
                detail::LUA_PREAMBLE_STARTED,
                detail::LUA_PREAMBLE_SUCCEEDED,
                detail::LUA_REPLY_BINDING_STARTED,
                detail::LUA_REPLY_BINDING_SUCCEEDED,
                detail::LUA_EPILOG_STARTED,
                detail::LUA_EPILOG_SUCCEEDED,
                detail::LUA_TEARDOWN_STARTED,
                detail::LUA_TEARDOWN_SUCCEEDED,
            ]
            .into_iter()
            .map(|detail| ("Gather".to_owned(), detail.to_owned()))
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
        SectionVm::new(Some(&failing_shared), &recorder, "Shared")
            .expect_err("shared execution must fail");
        assert_eq!(
            recorder.observations(),
            [
                detail::LUA_SHARED_LOAD_STARTED,
                detail::LUA_SHARED_LOAD_FAILED,
            ]
            .into_iter()
            .map(|detail| ("Shared".to_owned(), detail.to_owned()))
            .collect::<Vec<_>>()
        );

        for (section, expected, operation) in [
            ("Preamble", detail::LUA_PREAMBLE_FAILED, 0_u8),
            ("Reply", detail::LUA_REPLY_BINDING_FAILED, 1_u8),
            ("Epilog", detail::LUA_EPILOG_FAILED, 2_u8),
        ] {
            let recorder = Recorder::default();
            let vm = SectionVm::new(None, &NullObserver, section).expect("VM must build");
            let error = match operation {
                0 => vm
                    .run_preamble(&program("return nil"), &recorder, section)
                    .expect_err("preamble before injection must fail"),
                1 => vm
                    .bind_reply("private reply", &recorder, section)
                    .expect_err("reply before injection must fail"),
                _ => vm
                    .run_epilog(&program("return nil"), &recorder, section)
                    .expect_err("epilog before injection must fail"),
            };
            assert!(error.to_string().contains("not been injected"));
            let observations = recorder.observations();
            assert_eq!(
                observations.last().map(|(_, detail)| detail.as_str()),
                Some(expected)
            );
            assert!(!format!("{observations:?}").contains("private reply"));
        }
    }

    #[test]
    fn lua_program_retains_source_and_round_trips_bytecode() {
        let source = "return greeting .. ' world'";
        let program =
            LuaProgram::compile(source, "section Gather preamble", &NullObserver, "Gather")
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
    fn malformed_lua_reports_location_and_retains_source_diagnostic() {
        let source = "local secret =\nreturn secret";
        let location = "section Gather preamble";
        let error = LuaProgram::compile(source, location, &NullObserver, "Gather")
            .expect_err("malformed Lua must not compile");

        match &error {
            Error::LuaCompile {
                location: actual_location,
                lua_source: actual_source,
                message,
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
        LuaProgram::compile(source, location, &recorder, "Gather").expect("valid Lua must compile");
        assert_eq!(
            recorder.observations(),
            vec![
                (
                    "Gather".to_owned(),
                    detail::LUA_COMPILATION_STARTED.to_owned(),
                ),
                (
                    "Gather".to_owned(),
                    detail::LUA_COMPILATION_SUCCEEDED.to_owned(),
                ),
            ]
        );

        let recorder = Recorder::default();
        LuaProgram::compile("local private =", location, &recorder, "Gather")
            .expect_err("malformed Lua must fail");
        let observations = recorder.observations();
        assert_eq!(
            observations,
            vec![
                (
                    "Gather".to_owned(),
                    detail::LUA_COMPILATION_STARTED.to_owned(),
                ),
                (
                    "Gather".to_owned(),
                    detail::LUA_COMPILATION_FAILED.to_owned(),
                ),
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
    fn single_add_records_its_names() {
        let out = run("tools.add('web_search', 'web_fetch')", "").unwrap();
        assert_eq!(out.scoped_tools, vec!["web_search", "web_fetch"]);
    }

    #[test]
    fn multiple_adds_accumulate_and_dedupe() {
        let out = run(
            "tools.add('web_search')\ntools.add('web_fetch', 'web_search')",
            "",
        )
        .unwrap();
        assert_eq!(out.scoped_tools, vec!["web_search", "web_fetch"]);
    }

    #[test]
    fn add_inside_if_branch_records() {
        let out = run("if true then tools.add('web_search') end", "").unwrap();
        assert_eq!(out.scoped_tools, vec!["web_search"]);
    }

    #[test]
    fn no_add_leaves_scoped_tools_empty() {
        let out = run("local x = 1", "").unwrap();
        assert!(out.scoped_tools.is_empty());
    }

    // --- The always-on `store` table ---

    #[test]
    fn store_write_then_read_returns_numbered_content() {
        let out = run(
            "store.write('a.txt', 'first\\nsecond')\nreturn store.read('a.txt')",
            "",
        )
        .unwrap();
        assert_eq!(out.returned.as_deref(), Some("1| first\n2| second"));
    }

    #[test]
    fn store_append_extends_the_file() {
        let out = run(
            "store.append('log.txt', 'one\\n')\nstore.append('log.txt', 'two')\nreturn store.read('log.txt')",
            "",
        )
        .unwrap();
        assert_eq!(out.returned.as_deref(), Some("1| one\n2| two"));
    }

    #[test]
    fn store_str_replace_edits_in_place() {
        let out = run(
            "store.write('a.txt', 'the quick brown fox')\nstore.str_replace('a.txt', 'quick', 'slow')\nreturn store.read('a.txt')",
            "",
        )
        .unwrap();
        assert_eq!(out.returned.as_deref(), Some("1| the slow brown fox"));
    }

    #[test]
    fn store_delete_then_read_raises() {
        let err = run(
            "store.write('a.txt', 'gone soon')\nstore.delete('a.txt')\nreturn store.read('a.txt')",
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
        let store = Store::memory();
        run_with("store.write('shared.txt', 'from lua')", &store).unwrap();
        assert_eq!(
            store.read("shared.txt").expect("read"),
            "1| from lua",
            "a Lua write must land in the shared store"
        );
    }

    #[test]
    fn store_reports_are_ordered_exact_and_payload_free_on_failure() {
        let recorder = Recorder::default();
        let store = Store::memory();
        let source = "store.write('secret/path.txt', 'private contents')\n\
                      store.read('secret/path.txt')\n\
                      store.str_replace('secret/path.txt', 'missing secret', 'replacement')";
        let error = run_chunk(
            source,
            "private input",
            &json!({ "id": 1, "when": "t" }),
            &store,
            &recorder,
            "Gather",
        )
        .expect_err("the missing anchor must fail");
        assert!(matches!(error, Error::Lua(_)));

        let observations = recorder.observations();
        assert_eq!(
            observations,
            vec![
                (
                    "Gather".to_string(),
                    detail::STORE_WRITE_SUCCEEDED.to_string(),
                ),
                (
                    "Gather".to_string(),
                    detail::STORE_READ_SUCCEEDED.to_string(),
                ),
                (
                    "Gather".to_string(),
                    detail::STORE_REPLACE_FAILED.to_string(),
                ),
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
    fn every_store_operation_reports_its_exact_success_and_failure() {
        struct Case {
            source: &'static str,
            success: &'static str,
            failure: &'static str,
            prepare: fn(&Store),
        }

        fn empty(_store: &Store) {}

        fn existing(store: &Store) {
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
                source: "store.read('a.txt')",
                success: detail::STORE_READ_SUCCEEDED,
                failure: detail::STORE_READ_FAILED,
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
            let store = Store::memory();
            (case.prepare)(&store);
            let recorder = Recorder::default();
            run_chunk(case.source, "", &json!({}), &store, &recorder, "Store")
                .expect("the memory store operation succeeds");
            assert_eq!(
                recorder.observations(),
                vec![("Store".to_owned(), case.success.to_owned())],
                "wrong success observation for {}",
                case.source
            );

            let store = Store::new(Box::new(FailingStore));
            let recorder = Recorder::default();
            let error = run_chunk(case.source, "", &json!({}), &store, &recorder, "Store")
                .expect_err("the failing backend rejects every operation");
            assert!(matches!(error, Error::Lua(_)));
            assert_eq!(
                recorder.observations(),
                vec![("Store".to_owned(), case.failure.to_owned())],
                "wrong failure observation for {}",
                case.source
            );
        }
    }

    #[test]
    fn store_observations_happen_before_later_lua_side_effects() {
        let store = Store::memory();
        let recorder = BoundaryRecorder {
            store: store.clone(),
            snapshots: Mutex::new(Vec::new()),
        };

        run_chunk(
            "store.write('first.txt', '')\nstore.write('second.txt', '')",
            "",
            &json!({}),
            &store,
            &recorder,
            "Store",
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
