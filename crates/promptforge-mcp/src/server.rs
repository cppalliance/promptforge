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
//! to correct it: an unresolvable prompt name carries the enabled names, a
//! refused admission names the wait, an unknown or evicted `run_id` names the
//! retention window, and a run that started and failed carries its error and
//! its whole `RunResult`. Everything left over is `-32603`.
//!
//! A call that outruns `reply_deadline` is answered with a `running` result
//! naming its `run_id`, while the run itself continues; `check_run` collects it
//! afterwards. [`runner`] holds that race and [`crate::registry`] the records
//! it leaves.

mod bind;
mod resolve;
mod runner;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode, ErrorData,
    Implementation, InitializeResult, JsonObject, ListToolsResult, PaginatedRequestParams,
    ProgressToken, ServerCapabilities, ServerInfo,
};
use rmcp::service::{NotificationContext, Peer, RequestContext, RoleServer};
use serde_json::{Value, json};

use crate::catalog::{Catalog, CatalogHandle, Entry};
use crate::config::Config;
use crate::registry::RunRegistry;
use crate::result::{RunResult, RunStatus};
use crate::retrieval::{Retrieval, Shortlist};
use crate::tools::{
    CHECK_RUN, LIST_PROMPTS, NEED_PROMPT, RUN_PROMPT, publishes_built_in, tool_definitions,
};
use crate::watch::Sessions;

/// What the session-level instructions tell a client once, so a model that
/// never reads a tool description still learns the one rule that matters: the
/// runner takes a name from the listing, not a guessed one.
const INSTRUCTIONS: &str = "This server runs PromptForge prompts. Some prompts have their own tool; the rest are reached with run_prompt. Call list_prompts to see every prompt this server can run, and pass run_prompt a name from that listing rather than guessing one.";

/// What a caller of `need_prompt` is told when the retrieval index is not
/// loaded: that this one tool cannot answer, and where an answer still is. The
/// tool was advertised, so the caller did nothing wrong and a protocol fault
/// would be blaming it for the server's own state.
const RETRIEVAL_UNAVAILABLE: &str = "need_prompt cannot answer: this server's retrieval index is not loaded. Call list_prompts and choose a prompt from the catalog instead.";

/// What a caller collecting an id nobody holds is told: that the id is unknown
/// and how long a finished run stays collectable, so a model that polled too
/// late learns why rather than reading it as a fault.
fn unknown_run(run_id: &str, retained: Duration) -> String {
    format!(
        "no run {run_id}. A run is collectable while it is going and for {} after it finishes; anything older has been evicted.",
        humantime::format_duration(retained)
    )
}

/// Who a run reports its progress to: the peer that asked for it, under the
/// token it supplied. Absent when the call carried no `progressToken`.
type Reporting = (Peer<RoleServer>, ProgressToken);

/// The MCP server: a configuration, the catalog it publishes, and the runs it
/// has started.
///
/// The first two are shared rather than owned, because the watcher replaces the
/// catalog underneath a live server and a run in flight keeps the snapshot it
/// started with. The registry is shared with every background run, which is what
/// lets one that outlived its call still record what it produced.
///
/// Cloning is how the HTTP transport gives each session a handler: every field
/// is shared, so a clone publishes the same catalog and, more importantly, the
/// same registry - a run started in one session is collectable from another -
/// and registers its session in the same list the watcher announces to.
#[derive(Debug, Clone)]
pub struct PromptForgeServer {
    config: Arc<Config>,
    catalog: Arc<CatalogHandle>,
    registry: Arc<RunRegistry>,
    sessions: Arc<Sessions>,
    retrieval: Arc<Retrieval>,
}

impl PromptForgeServer {
    /// Builds a server over a configuration, a live catalog, the session list a
    /// reload announces a changed tool set to, and the index `need_prompt`
    /// answers from.
    ///
    /// The run registry is built from `[server]` here rather than passed in:
    /// its limits are that table's, and one server has exactly one. The session
    /// list and the retrieval index are passed in, because the watcher holds the
    /// other end of both - it announces to the one and rebuilds the other.
    /// [`Retrieval::idle`] is the retrieval of a server that publishes no
    /// `need_prompt` or could not load its model.
    #[must_use]
    pub fn new(
        config: Arc<Config>,
        catalog: Arc<CatalogHandle>,
        sessions: Arc<Sessions>,
        retrieval: Arc<Retrieval>,
    ) -> PromptForgeServer {
        let registry = Arc::new(RunRegistry::new(&config.server));
        PromptForgeServer {
            config,
            catalog,
            registry,
            sessions,
            retrieval,
        }
    }

    /// Answers one `tools/call` that asked for no progress.
    ///
    /// This is the whole of the handler's behavior, separated from
    /// [`ServerHandler::call_tool`] so it can be driven directly.
    ///
    /// # Errors
    /// The same as [`dispatch_with_progress`](Self::dispatch_with_progress).
    pub async fn dispatch(
        &self,
        request: CallToolRequestParams,
    ) -> Result<CallToolResult, ErrorData> {
        self.answer(request, None).await
    }

    /// Answers one `tools/call`, reporting the run to `peer` under `token`.
    ///
    /// The caller supplies both from the request's `progressToken`; a call that
    /// carried none goes to [`dispatch`](Self::dispatch) instead, and the two
    /// differ in nothing but the notifications.
    ///
    /// # Errors
    /// Returns `-32602` when the arguments are not the shape the tool's schema
    /// declares, `-32601` when the named tool is not one this catalog
    /// publishes, and `-32603` when the result cannot be assembled. A failure
    /// the calling model can act on is not an error here: it is an `Ok` result
    /// carrying `isError`.
    pub async fn dispatch_with_progress(
        &self,
        request: CallToolRequestParams,
        peer: Peer<RoleServer>,
        token: ProgressToken,
    ) -> Result<CallToolResult, ErrorData> {
        self.answer(request, Some((peer, token))).await
    }

    /// Routes one call and answers it.
    ///
    /// The built-ins are matched before the catalog, and each only while this
    /// catalog publishes it: a built-in absent from `tools/list` is a name that
    /// does not exist here, not one the handler answers anyway.
    ///
    /// # Errors
    /// The same as [`dispatch_with_progress`](Self::dispatch_with_progress).
    async fn answer(
        &self,
        request: CallToolRequestParams,
        reporting: Option<Reporting>,
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
                    Ok(entry) => self.run(entry, &args, reporting).await,
                    Err(message) => Ok(text_error(message)),
                }
            }
            CHECK_RUN if publishes_built_in(&catalog, name) => {
                let run_id = required_string(arguments, "run_id")?;
                match self.registry.check(&run_id) {
                    Some(result) => run_result(&result),
                    None => Ok(text_error(unknown_run(
                        &run_id,
                        self.registry.retain_completed(),
                    ))),
                }
            }
            NEED_PROMPT if publishes_built_in(&catalog, name) => {
                let capability = required_string(arguments, "capability")?;
                // Embedding the capability is a transformer forward pass, which
                // has no business on a runtime worker. The index behind it is
                // swapped atomically, so the task either ranks against the
                // index that was current when it started or against the one
                // that replaced it, never against a half-built one.
                let retrieval = Arc::clone(&self.retrieval);
                let shortlist =
                    tokio::task::spawn_blocking(move || retrieval.shortlist(&capability))
                        .await
                        .map_err(|e| {
                            ErrorData::internal_error(
                                format!("rank prompts for the capability: {e}"),
                                None,
                            )
                        })?;
                need_prompt_result(&shortlist)
            }
            _ => match catalog.find(name) {
                Some(entry) if entry.is_direct() => {
                    let args = optional_string(arguments, "args")?;
                    self.run(entry, &args, reporting).await
                }
                _ => Err(ErrorData::new(
                    ErrorCode::METHOD_NOT_FOUND,
                    format!("no tool named {name}"),
                    None,
                )),
            },
        }
    }

    /// Runs one catalog entry and reports it, either as its result or as a
    /// `run_id` to collect the result by.
    ///
    /// See [`runner`] for the gates a run passes and what the reply deadline
    /// does to a slow one.
    async fn run(
        &self,
        entry: &Entry,
        args: &str,
        reporting: Option<Reporting>,
    ) -> Result<CallToolResult, ErrorData> {
        runner::run(&self.config, &self.registry, entry, args, reporting).await
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

    /// Registers the session, so a reload that changes the tool set can tell it.
    ///
    /// This is the one hook that names a live client. The list is kept here
    /// rather than in the transport because stdio has no session manager to ask,
    /// and a handler clone per session is exactly one registration per session.
    fn on_initialized(
        &self,
        context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.sessions.register(context.peer);
        std::future::ready(())
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
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // A `progressToken` is the client saying it will render progress. With
        // no token there is nobody to notify, so there is no channel and no
        // pump either.
        let result = match context.meta.get_progress_token() {
            Some(token) => {
                self.dispatch_with_progress(request, context.peer, token)
                    .await
            }
            None => self.dispatch(request).await,
        };
        result.map(CallToolResponse::Complete)
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

/// The candidates a capability retrieved, shaped like the listing so a caller
/// reads one thing whichever way it found a prompt.
///
/// An empty shortlist is a success, not an error: "no prompt is close to this"
/// is an answer, and the catalog is one `list_prompts` call away.
///
/// # Errors
/// Returns `-32603` when the engine could not embed the capability, which is
/// nothing the caller can correct, and when the candidates cannot be serialized.
fn need_prompt_result(shortlist: &Shortlist) -> Result<CallToolResult, ErrorData> {
    let candidates = match shortlist {
        Shortlist::Candidates(candidates) => candidates,
        Shortlist::Unavailable => return Ok(text_error(RETRIEVAL_UNAVAILABLE.to_owned())),
        Shortlist::Failed(detail) => {
            return Err(ErrorData::internal_error(
                format!("rank prompts for the capability: {detail}"),
                None,
            ));
        }
    };
    let structured = json!({ "prompts": candidates });
    let text = serde_json::to_string_pretty(&structured)
        .map_err(|e| ErrorData::internal_error(format!("render the candidates: {e}"), None))?;
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
