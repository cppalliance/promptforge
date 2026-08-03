//! The MCP handler: what the server answers `tools/list` and `tools/call` with.
//!
//! A call arrives either at a prompt's own tool or at the runner, and both end
//! in the same place - one prompt run against the configured gateway, reported
//! as a [`RunResult`]. The listing tool answers from the catalog snapshot the
//! call loaded, so a prompt written since the client connected is already in
//! it.
//!
//! Where a failure lands is decided by who can fix it. A malformed argument
//! shape is the client's own bug and comes back as `-32602`, which the calling
//! model never sees and never could act on. Anything the model itself can
//! correct comes back as a result with `isError` set and the information needed
//! to correct it: an unresolvable prompt name carries the enabled names, and a
//! run that started and failed carries its error and its whole `RunResult`.
//! Everything left over is `-32603`.

mod bind;
mod resolve;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Instant;

use promptforge_core::execute::{self, RunOptions};
use promptforge_core::observe::NullObserver;
use promptforge_core::store::Store;
use promptforge_core::tools::Tool;
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode, ErrorData,
    Implementation, InitializeResult, JsonObject, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use serde_json::{Value, json};

use crate::catalog::{Catalog, CatalogHandle, Entry};
use crate::config::Config;
use crate::result::{RunResult, RunStatus};
use crate::tools::{
    CHECK_RUN, LIST_PROMPTS, NEED_PROMPT, RUN_PROMPT, publishes_built_in, tool_definitions,
};

/// What the session-level instructions tell a client once, so a model that
/// never reads a tool description still learns the one rule that matters: the
/// runner takes a name from the listing, not a guessed one.
const INSTRUCTIONS: &str = "This server runs PromptForge prompts. Some prompts have their own tool; the rest are reached with run_prompt. Call list_prompts to see every prompt this server can run, and pass run_prompt a name from that listing rather than guessing one.";

/// The message a built-in that is published but not yet answered returns, so a
/// caller reads why rather than a protocol fault.
fn not_yet_answered(tool: &str) -> String {
    format!("{tool} is published but not answered yet by this build of the server.")
}

/// How many model round trips a run reports until there is an observer counting
/// them. Step 7 replaces the [`NullObserver`] this run uses, and the count with
/// it; `tests::step_7_must_replace_the_uncounted_turn_total` asserts this zero
/// so the substitution cannot be forgotten in silence.
const UNCOUNTED_TURNS: u32 = 0;

/// The MCP server: a configuration, and the catalog it publishes.
///
/// Both are shared rather than owned, because the watcher replaces the catalog
/// underneath a live server and a run in flight keeps the snapshot it started
/// with.
#[derive(Debug)]
pub struct PromptForgeServer {
    config: Arc<Config>,
    catalog: Arc<CatalogHandle>,
}

impl PromptForgeServer {
    /// Builds a server over a configuration and a live catalog.
    #[must_use]
    pub fn new(config: Arc<Config>, catalog: Arc<CatalogHandle>) -> PromptForgeServer {
        PromptForgeServer { config, catalog }
    }

    /// Answers one `tools/call`, without the transport around it.
    ///
    /// This is the whole of the handler's behavior, separated from
    /// [`ServerHandler::call_tool`] so it can be driven directly.
    ///
    /// The built-ins are matched before the catalog, and each only while this
    /// catalog publishes it: a built-in absent from `tools/list` is a name that
    /// does not exist here, not one the handler answers anyway.
    ///
    /// # Errors
    /// Returns `-32602` when the arguments are not the shape the tool's schema
    /// declares, `-32601` when the named tool is not one this catalog
    /// publishes, and `-32603` when the result cannot be assembled. A failure
    /// the calling model can act on is not an error here: it is an `Ok` result
    /// carrying `isError`.
    pub async fn dispatch(
        &self,
        request: CallToolRequestParams,
    ) -> Result<CallToolResult, ErrorData> {
        let catalog = self.catalog.load();
        let arguments = request.arguments.as_ref();
        let name = request.name.as_ref();
        match name {
            LIST_PROMPTS if publishes_built_in(&catalog, name) => list_prompts_result(&catalog),
            RUN_PROMPT if publishes_built_in(&catalog, name) => {
                let requested = required_string(arguments, "prompt")?;
                let args = optional_string(arguments, "args")?;
                match resolve::resolve(&catalog, &requested) {
                    Ok(entry) => self.run(entry, &args).await,
                    Err(message) => Ok(text_error(message)),
                }
            }
            CHECK_RUN | NEED_PROMPT if publishes_built_in(&catalog, name) => {
                Ok(text_error(not_yet_answered(name)))
            }
            _ => match catalog.find(name) {
                Some(entry) if entry.is_direct() => {
                    let args = optional_string(arguments, "args")?;
                    self.run(entry, &args).await
                }
                _ => Err(ErrorData::new(
                    ErrorCode::METHOD_NOT_FOUND,
                    format!("no tool named {name}"),
                    None,
                )),
            },
        }
    }

    /// Runs one catalog entry and reports it.
    ///
    /// A broken entry never runs: its recorded problem becomes the failure,
    /// which is what makes a prompt that stopped parsing say so instead of
    /// quietly running its last good copy.
    async fn run(&self, entry: &Entry, args: &str) -> Result<CallToolResult, ErrorData> {
        let run_id = new_run_id();
        let started = Instant::now();

        let Some(prompt) = entry.prompt() else {
            let problem = entry
                .problem()
                .unwrap_or("the prompt is unavailable")
                .to_owned();
            return run_result(&RunResult::failed(
                run_id,
                entry.name(),
                entry.version(),
                problem,
                UNCOUNTED_TURNS,
                0,
            ));
        };

        let owned = match bind::select_tools(&prompt.frontmatter.tools, &self.config.gateway) {
            Ok(tools) => tools,
            Err(message) => {
                return run_result(&RunResult::failed(
                    run_id,
                    entry.name(),
                    entry.version(),
                    message,
                    UNCOUNTED_TURNS,
                    elapsed_ms(started),
                ));
            }
        };
        let tools: Vec<&dyn Tool> = owned.iter().map(AsRef::as_ref).collect();

        let store = Store::memory();
        // Step 7 replaces this with the observer that forwards the run's events
        // as progress notifications and counts its turns; until then a run
        // reports nothing while it is in flight and no turn total after it.
        let options = RunOptions {
            observer: &NullObserver,
            client: Some(bind::gateway_client(&self.config.gateway)),
        };
        let outcome = execute::run(prompt, args, &tools, &store, options).await;

        let elapsed = elapsed_ms(started);
        match outcome {
            Ok(value) => run_result(&RunResult::completed(
                run_id,
                entry.name(),
                entry.version(),
                value,
                UNCOUNTED_TURNS,
                elapsed,
            )),
            Err(error) => run_result(&RunResult::failed(
                run_id,
                entry.name(),
                entry.version(),
                error.to_string(),
                UNCOUNTED_TURNS,
                elapsed,
            )),
        }
    }
}

impl ServerHandler for PromptForgeServer {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_server_info(Implementation::new(
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tool_definitions(
            &self.catalog.load(),
        )))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.dispatch(request).await.map(CallToolResponse::Complete)
    }
}

/// The listing every enabled prompt appears in, healthy or broken.
///
/// # Errors
/// Returns `-32603` if the listing cannot be serialized.
fn list_prompts_result(catalog: &Catalog) -> Result<CallToolResult, ErrorData> {
    let prompts: Vec<Value> = catalog
        .entries()
        .iter()
        .map(|entry| {
            json!({
                "name": entry.name(),
                "description": entry.description(),
                "version": entry.version(),
                "direct": entry.is_direct(),
                "problem": entry.problem(),
            })
        })
        .collect();
    let structured = json!({ "prompts": prompts });
    let text = serde_json::to_string_pretty(&structured)
        .map_err(|e| ErrorData::internal_error(format!("render the prompt listing: {e}"), None))?;
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(structured);
    Ok(result)
}

/// A run reported as a tool result: the value or the error verbatim in the text
/// block, the whole record in `structuredContent`, and `isError` set when the
/// run failed.
///
/// # Errors
/// Returns `-32603` if the record cannot be serialized.
fn run_result(run: &RunResult) -> Result<CallToolResult, ErrorData> {
    let text = run.text();
    let failed = matches!(run.status, RunStatus::Failed);
    let structured = serde_json::to_value(run)
        .map_err(|e| ErrorData::internal_error(format!("render the run result: {e}"), None))?;
    let content = vec![ContentBlock::text(text)];
    let mut result = if failed {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    };
    result.structured_content = Some(structured);
    Ok(result)
}

/// A result the calling model is meant to read and act on.
fn text_error(message: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

/// A required string argument.
///
/// # Errors
/// Returns `-32602` when the argument is absent or is not a string, since the
/// tool's schema declared both.
fn required_string(arguments: Option<&JsonObject>, key: &str) -> Result<String, ErrorData> {
    match arguments.and_then(|arguments| arguments.get(key)) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(ErrorData::invalid_params(
            format!("{key} must be a string"),
            None,
        )),
        None => Err(ErrorData::invalid_params(
            format!("{key} is required"),
            None,
        )),
    }
}

/// An optional string argument; absent is the empty string.
///
/// # Errors
/// Returns `-32602` when the argument is present but is not a string.
fn optional_string(arguments: Option<&JsonObject>, key: &str) -> Result<String, ErrorData> {
    match arguments.and_then(|arguments| arguments.get(key)) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(ErrorData::invalid_params(
            format!("{key} must be a string"),
            None,
        )),
    }
}

/// A fresh run identifier: 128 random bits in hex, which is unguessable enough
/// for a value that only has to be unique within one process's lifetime.
fn new_run_id() -> String {
    format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..))
}

/// Milliseconds since `started`, saturating rather than wrapping.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
