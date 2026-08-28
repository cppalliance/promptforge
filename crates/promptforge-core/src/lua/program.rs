use super::{Error, Function, Lua, LuaOptions, NonZeroU32, Observer, Result, StdLib, detail};

/// Identifies whether temporary compiler setup or chunk compilation failed.
enum CompilerError {
    Vm(mlua::Error),
    Chunk(mlua::Error),
}

/// Compiles a named chunk in the fixed temporary VM and keeps debug info.
fn compile_chunk(source: &str, location: &str) -> std::result::Result<Vec<u8>, CompilerError> {
    let lua = Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH,
        LuaOptions::default(),
    )
    .map_err(CompilerError::Vm)?;
    let function = lua
        .load(source)
        .set_name(location)
        .into_function()
        .map_err(CompilerError::Chunk)?;
    // Keep debug info so runtime errors report the chunk name and line
    // (`dump(true)` strips them and leaves `?:` in the traceback).
    Ok(function.dump(false))
}

/// Compiled Lua 5.5 source that can be loaded into multiple process-local VMs.
///
/// A program retains its original source for diagnostics and stores bytecode
/// produced once by Lua 5.5. The bytecode is an in-memory implementation detail:
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
/// let prompt = Prompt::parse(source, "doc", &NullObserver::default())?;
/// let program = prompt.sections()[0]
///     .prologue()
///     .ok_or("the section has a Lua prologue")?;
/// assert_eq!(program.source(), "return 1");
/// assert!(program.source_line().get() >= 1);
/// assert!(program.location().contains("Only"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LuaProgram {
    source: String,
    pub(crate) bytecode: Vec<u8>,
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
    /// Compiles `source` as Lua 5.5 bytecode without executing it.
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
    ///     &NullObserver::default(),
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

        let bytecode = match compile_chunk(source, location) {
            Ok(bytecode) => bytecode,
            Err(CompilerError::Vm(error)) => {
                observer.observe(execution, section, detail::LUA_COMPILATION_FAILED);
                return Err(Error::lua(error));
            }
            Err(CompilerError::Chunk(error)) => {
                observer.observe(execution, section, detail::LUA_COMPILATION_FAILED);
                return Err(Error::LuaCompile {
                    location: location.to_owned(),
                    source_line: source_line.get(),
                    lua_source: source.to_owned(),
                    message: error.to_string(),
                    source: Box::new(error),
                });
            }
        };

        observer.observe(execution, section, detail::LUA_COMPILATION_SUCCEEDED);
        Ok(Self {
            source: source.to_owned(),
            bytecode,
            location: location.to_owned(),
            source_line,
        })
    }

    /// Returns a compiled empty chunk standing in for an absent shared library.
    ///
    /// Section startup replays the shared library unconditionally; a prompt
    /// without a `lua shared` fence replays this chunk instead, so the
    /// startup sequence carries no `Option` branch. The compilation is
    /// internal bookkeeping and emits no observations.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the temporary compiler VM cannot be created.
    pub(crate) fn empty() -> Result<Self> {
        let bytecode = compile_chunk("", "shared library").map_err(|error| match error {
            CompilerError::Vm(error) | CompilerError::Chunk(error) => Error::lua(error),
        })?;
        Ok(Self {
            source: String::new(),
            bytecode,
            location: "shared library".to_owned(),
            source_line: NonZeroU32::MIN,
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
            .map_err(Error::lua)
    }

    /// Maps a Lua runtime failure to its ordered core outcome.
    ///
    /// Cancellation is checked first and returns [`Error::Interrupted`].
    /// Otherwise a recognized host quota returns [`Error::LuaQuota`]. All
    /// remaining failures return [`Error::LuaRuntime`] with this program's
    /// chunk-relative line rewritten to an absolute prompt-source line. Nested
    /// errors from other chunks (for example a fanout arm) are left unchanged.
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
        // Retain the originating `mlua` error as the private source (F4) with the
        // mapped prompt-location message, instead of flattening it to a string.
        Error::LuaRuntime {
            message: mapped,
            source: Box::new(error.clone()),
        }
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
pub(crate) fn quota_resource(raw: &str) -> Option<&'static str> {
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
pub(crate) fn map_chunk_line_to_absolute(
    message: &str,
    source_line: NonZeroU32,
    location: &str,
) -> String {
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
