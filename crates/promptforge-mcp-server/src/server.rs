//! The MCP handler: what the server answers `tools/list` and `tools/call` with.
//!
//! Every call arrives at one of the built-ins, and a prompt is run only because
//! the runner was handed its name. The listing tool answers from the catalog
//! snapshot the call loaded, so a prompt written since the client connected is
//! already in it.
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
mod listing;
mod reply;
mod resolve;
mod runner;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ErrorCode, ErrorData, Implementation,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ProgressToken, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::{Peer, RequestContext, RoleServer};

use crate::catalog::{CatalogHandle, Entry};
use crate::config::Config;
use crate::registry::RunRegistry;
use crate::tools::{BuiltInTool, prompt_value, tool_definitions};

#[cfg(feature = "picker")]
pub(crate) use self::listing::need_prompt_result;
use self::listing::{PAGE_LIMIT, list_prompts_result, page_start};
use self::reply::{optional_string, required_string, run_result, text_error, unknown_run};

pub use bind::PreparedTools;

/// What the session-level instructions tell a client once, so a model that
/// never reads a tool description still learns the two rules that matter: this
/// server executes a prompt the caller names, and what comes back is the
/// artifact the user asked for rather than material to work from.
///
/// It is written in the register of a command interpreter. It makes no claim on
/// any situation and invites no selection, because a prompt is a command and a
/// model that never calls this server is behaving correctly.
pub(crate) const INSTRUCTIONS: &str = concat!(
    "This server executes PromptForge prompts. It runs a prompt only when a caller names one: list_prompts reports the names it can run, and run_prompt takes one of those names. ",
    prompt_value!()
);

/// The most a `capability` may weigh before ranking refuses it.
///
/// A capability is a short imperative phrase, and embedding it is a transformer
/// forward pass on a blocking worker; a multi-kilobyte capability is a defect or
/// an abuse, not a phrasing, and letting one onto the blocking pool spends CPU
/// no result will justify. The bound is checked before the string is moved onto
/// the task, so an over-long input costs nothing. Four kilobytes is far above
/// any real capability and far below a size that would strain the embedder.
#[cfg(feature = "picker")]
const MAX_CAPABILITY_LEN: usize = 4096;

/// How many `need_prompt` rankings may embed a capability at once.
///
/// Each ranking is a transformer forward pass on a blocking worker. The input
/// cap above stops one call from being oversized, but says nothing about how
/// many run together: left unbounded, a burst of concurrent `need_prompt` calls
/// would each schedule onto Tokio's unbounded blocking pool and could exhaust
/// it, starving every other blocking task. This process-wide bound caps how
/// many ranks are in flight; excess callers wait for a permit rather than
/// piling onto the pool. It is deliberately small - `need_prompt` is an
/// occasional discovery call, not a hot path, and CPU embedding gains nothing
/// from oversubscription.
#[cfg(feature = "picker")]
const MAX_CONCURRENT_RANKS: usize = 4;

/// The process-wide admission bound for ranking work, acquired before a rank is
/// scheduled onto the blocking pool. `const_new` so it needs no lazy init and
/// is shared across every handler clone and session.
#[cfg(feature = "picker")]
static RANK_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_CONCURRENT_RANKS);

/// Who a run reports its progress to: the peer that asked for it, under the
/// token it supplied. Absent when the call carried no `progressToken`.
type Reporting = (Peer<RoleServer>, ProgressToken);

/// The MCP server: a configuration, the live generation it publishes, and the
/// runs it has started.
///
/// The first two are shared rather than owned, because the watcher replaces the
/// generation underneath a live server and a run in flight keeps the snapshot it
/// started with. The registry is shared with every background run, which is what
/// lets one that outlived its call still record what it produced.
///
/// Cloning is how the HTTP transport gives each session a handler: every field
/// is shared, so a clone publishes the same generation and, more importantly,
/// the same registry - a run started in one session is collectable from another.
#[derive(Debug, Clone)]
pub struct PromptForgeServer {
    config: Arc<Config>,
    catalog: Arc<CatalogHandle>,
    registry: Arc<RunRegistry>,
    tools: Arc<PreparedTools>,
}

impl PromptForgeServer {
    /// Builds a server over a configuration and a live generation.
    ///
    /// The run registry is built from `[server]` here rather than passed in:
    /// its limits are that table's, and one server has exactly one. The catalog
    /// the runner resolves names in and the index `need_prompt` ranks against
    /// are both read from the same [`CatalogHandle`] snapshot, so the watcher
    /// replacing one replaces both together.
    ///
    /// # Examples
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use promptforge_mcp_server::{
    /// #     Catalog, CatalogHandle, Config, OnBroken, PreparedTools, PromptForgeServer,
    /// # };
    /// # async fn demo(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    /// let config = Arc::new(config);
    /// let catalog = Catalog::resolve(&config, OnBroken::Reject)?;
    /// let tools = Arc::new(PreparedTools::load(&config).await?);
    /// let server = PromptForgeServer::new(
    ///     Arc::clone(&config),
    ///     Arc::new(CatalogHandle::new(catalog)),
    ///     tools,
    /// );
    /// # let _ = server;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn new(
        config: Arc<Config>,
        catalog: Arc<CatalogHandle>,
        tools: Arc<PreparedTools>,
    ) -> PromptForgeServer {
        let registry = Arc::new(RunRegistry::new(&config.server));
        PromptForgeServer {
            config,
            catalog,
            registry,
            tools,
        }
    }

    /// Answers one `tools/call` that asked for no progress.
    ///
    /// This is the whole of the handler's behavior, separated from
    /// [`ServerHandler::call_tool`] so it can be driven directly.
    ///
    /// # Errors
    /// The same as [`dispatch_with_progress`](Self::dispatch_with_progress).
    pub(crate) async fn dispatch(
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
    pub(crate) async fn dispatch_with_progress(
        &self,
        request: CallToolRequestParams,
        peer: Peer<RoleServer>,
        token: ProgressToken,
    ) -> Result<CallToolResult, ErrorData> {
        self.answer(request, Some((peer, token))).await
    }

    /// Routes one call and answers it.
    ///
    /// Only a built-in this build publishes is routed: a name absent from
    /// `tools/list` - a prompt's own name included - does not exist here, and
    /// is not one the handler answers anyway.
    ///
    /// # Errors
    /// The same as [`dispatch_with_progress`](Self::dispatch_with_progress).
    async fn answer(
        &self,
        request: CallToolRequestParams,
        reporting: Option<Reporting>,
    ) -> Result<CallToolResult, ErrorData> {
        let generation = self.catalog.load();
        let arguments = request.arguments.as_ref();
        let name = request.name.as_ref();
        // Route on the single built-in enum. A name this build does not publish
        // - an unknown tool, a prompt's own name, or `need_prompt` without the
        // `picker` feature - resolves to no handler and is a method that does
        // not exist.
        let Some(tool) = BuiltInTool::from_name(name).filter(|tool| tool.published()) else {
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("no tool named {name}"),
                None,
            ));
        };
        // Exhaustive over `BuiltInTool`: adding a variant does not compile until
        // it gains a handler arm here, so a published built-in can never lack a
        // dispatcher and a handler can never answer a call the listing never
        // advertised.
        match tool {
            BuiltInTool::ListPrompts => {
                let cursor = optional_string(arguments, "cursor")?;
                list_prompts_result(generation.catalog(), &cursor)
            }
            BuiltInTool::RunPrompt => {
                let requested = required_string(arguments, "prompt")?;
                let args = optional_string(arguments, "args")?;
                let input_file = optional_string(arguments, "input_file")?;
                let input_text = optional_string(arguments, "input_text")?;
                let output_file = optional_string(arguments, "output_file")?;
                if !input_file.is_empty() && !input_text.is_empty() {
                    return Err(ErrorData::invalid_params(
                        "input_file and input_text are mutually exclusive; specify one, not both",
                        None,
                    ));
                }
                match resolve::resolve(generation.catalog(), &requested) {
                    Ok(entry) => {
                        self.run(entry, &args, &input_file, &input_text, &output_file, reporting)
                            .await
                    }
                    // A miss is caller-correctable: hand back every enabled
                    // name, nearest first, so the model's next call can name one
                    // exactly.
                    Err(resolve::ResolveError::NotFound) => {
                        let wanted = resolve::normalize(&requested);
                        Ok(text_error(format!(
                            "there is no prompt named \"{requested}\", so nothing was run. {}",
                            resolve::nearest_first(generation.catalog(), &wanted)
                        )))
                    }
                    // Ambiguity is not: it can only arise if the catalog admitted
                    // two prompts under one normalized name, an invariant its
                    // construction forbids, so it is an internal fault rather
                    // than a result the model can act on.
                    Err(resolve::ResolveError::Ambiguous) => Err(ErrorData::internal_error(
                        format!(
                            "the prompt name \"{requested}\" matched more than one enabled prompt"
                        ),
                        None,
                    )),
                }
            }
            BuiltInTool::CheckRun => {
                let run_id = required_string(arguments, "run_id")?;
                match self.registry.check(&run_id) {
                    Some(result) => run_result(&result),
                    None => Ok(text_error(unknown_run(
                        &run_id,
                        self.registry.retain_completed(),
                    ))),
                }
            }
            BuiltInTool::NeedPrompt => {
                #[cfg(feature = "picker")]
                {
                    let capability = required_string(arguments, "capability")?;
                    if capability.len() > MAX_CAPABILITY_LEN {
                        // Refused before the string reaches the blocking pool, so
                        // an over-long capability never costs a forward pass. It
                        // is a result the calling model can act on - shorten the
                        // phrase to the imperative the tool asks for - not a
                        // fault.
                        return Ok(text_error(format!(
                            "capability is {} bytes, over the {MAX_CAPABILITY_LEN}-byte limit; state it as one short imperative phrase.",
                            capability.len()
                        )));
                    }
                    // Bound how many ranks run at once before scheduling this
                    // one. The permit is acquired asynchronously, so the reactor
                    // is never blocked and an excess caller waits for a slot
                    // rather than spawning onto the unbounded blocking pool; it
                    // is moved into the blocking task so it is held for exactly
                    // the forward pass and returned the instant the rank ends.
                    // The semaphore is never closed, so the acquire cannot fail
                    // in practice.
                    let permit = RANK_SLOTS.acquire().await.map_err(|e| {
                        ErrorData::internal_error(format!("acquire a ranking slot: {e}"), None)
                    })?;
                    // Embedding the capability is a transformer forward pass,
                    // which has no business on a runtime worker. The generation
                    // is loaded once and moved onto the task, so it ranks against
                    // the index built over exactly the catalog it holds - and a
                    // reload that lands mid-call publishes a whole new generation
                    // this one never sees, never a half-built index.
                    let generation = Arc::clone(&generation);
                    let shortlist = tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        generation.shortlist(&capability)
                    })
                    .await
                    .map_err(|e| {
                        ErrorData::internal_error(
                            format!("rank prompts for the capability: {e}"),
                            None,
                        )
                    })?;
                    need_prompt_result(&shortlist)
                }
                #[cfg(not(feature = "picker"))]
                {
                    // Filtered out above by `published()` in a build without the
                    // `picker` feature, so this arm is unreachable; it exists
                    // only to keep the dispatch `match` exhaustive over
                    // `BuiltInTool`.
                    Err(ErrorData::new(
                        ErrorCode::METHOD_NOT_FOUND,
                        format!("no tool named {name}"),
                        None,
                    ))
                }
            }
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
        input_file: &str,
        input_text: &str,
        output_file: &str,
        reporting: Option<Reporting>,
    ) -> Result<CallToolResult, ErrorData> {
        runner::run(
            &self.config,
            &self.registry,
            Arc::clone(&self.tools),
            entry,
            args,
            input_file,
            input_text,
            output_file,
            reporting,
        )
        .await
    }

    /// Builds one page of the tool listing.
    ///
    /// This is the whole of what [`ServerHandler::list_tools`] answers,
    /// separated so it can be driven directly. It reads the fixed built-in set
    /// through [`tool_definitions`] and never the catalog `self` holds: the
    /// published tools are the same set for every catalog, so a prompt is
    /// reached only by naming it to `run_prompt`, never by appearing here.
    ///
    /// # Errors
    /// Returns `-32602` when `cursor` is not a cursor a previous page returned.
    // Deliberately a `&self` method that reads nothing from `self`: it mirrors
    // the `&self` `ServerHandler::list_tools` it backs, and its independence
    // from the catalog `self` holds is the exact property the tool-surface tests
    // drive it on two different catalogs to prove.
    #[allow(clippy::unused_self)]
    pub(crate) fn list_page(&self, cursor: Option<&str>) -> Result<ListToolsResult, ErrorData> {
        // The built-in set is at most four, so one page holds it whole and a
        // cursor is only ever exhausted; honoring it anyway keeps the tool the
        // protocol expects rather than one that quietly ignores a parameter.
        let all = tool_definitions();
        let start = page_start(cursor)?;
        let end = start.saturating_add(PAGE_LIMIT).min(all.len());
        let page = all.get(start..end).unwrap_or(&[]).to_vec();
        let mut result = ListToolsResult::with_all_items(page);
        if end < all.len() {
            result.next_cursor = Some(end.to_string());
        }
        Ok(result)
    }
}

impl ServerHandler for PromptForgeServer {
    /// The tool-list capability is advertised without `listChanged`, which is
    /// the honest answer: the published set is the same four built-ins for the
    /// life of the process, so there is nothing a client could be told.
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.list_page(request.and_then(|request| request.cursor).as_deref())
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

#[cfg(all(test, feature = "picker"))]
mod admission {
    use super::{MAX_CONCURRENT_RANKS, RANK_SLOTS};

    #[test]
    fn ranking_admission_is_bounded_to_a_small_permit_count() {
        // Every permit taken, the next attempt cannot proceed until one returns:
        // a burst of need_prompt calls waits for a slot rather than each spawning
        // onto the unbounded blocking pool.
        let mut held = Vec::new();
        for _ in 0..MAX_CONCURRENT_RANKS {
            held.push(
                RANK_SLOTS
                    .try_acquire()
                    .expect("a permit is free within the bound"),
            );
        }
        assert!(
            RANK_SLOTS.try_acquire().is_err(),
            "ranking beyond the bound waits for a returned permit"
        );
        drop(held);
        assert!(
            RANK_SLOTS.try_acquire().is_ok(),
            "a returned permit admits the next ranking"
        );
    }
}
