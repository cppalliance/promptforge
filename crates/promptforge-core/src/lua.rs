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
use crate::{Error, Result};

/// How many instructions between hook firings.
const HOOK_INTERVAL: u32 = 10_000;
/// Maximum number of hook firings before a block is aborted (~1e7 instructions).
const HOOK_BUDGET: u64 = 1_000;

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
        install_tools_table(&self.lua, &globals, &self.scoped_tools)?;
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
    /// Returns [`Error::Lua`] if host values have not been injected or the
    /// reply cannot be installed.
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
