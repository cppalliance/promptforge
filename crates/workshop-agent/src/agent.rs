//! `run_agent` and its leaf-dispatch driver.
//!
//! An agent program is one Lua chunk run as one coroutine on an agent VM -
//! a [`SectionVm`] built with the section construction sequence (harden,
//! untrusted, host injection, store, log, var) minus the section control
//! surface. The shared kernel is `models.infer` and `tool_call`; the
//! agent-only `models.chat` is installed here and nowhere else; `execute`,
//! `fanout`, and `jump` are absent, not stubbed, so touching them is an
//! undefined-global failure. The driver is leaf dispatch only: it resumes
//! the coroutine, validates each yield into a [`Request`], awaits exactly
//! one future - the current request - and resumes with the answer. Tool
//! dispatch goes through the shared [`dispatch_tool`] body; nothing here
//! duplicates it. Before every resume the driver republishes the
//! `runtime.events()` length bound ([`EventsSnapshot::refresh`]) - the
//! resume-refresh rule: appends land in the program's view only at
//! host-call resumes, never mid-chunk, so reads between suspensions stay
//! deterministic.
//!
//! `run_agent` installs [`AgentConfig::cancel`] as the task's cancel scope:
//! suspended host calls race cancellation, and running Lua observes the
//! same flag through the VM's instruction hook. Teardown is observed like a
//! section's, under the agent's name as the `section` label.

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use promptforge_core_support::cancel;
use promptforge_core_support::events::{CallMetrics, EventLog, ToolCallEvent};
use promptforge_core_support::observe::{Observer, detail};
use promptforge_core_support::untrusted::GuardNonce;
use promptforge_lua::{
    Answer, ChatResult, CoroStep, Error as LuaError, EventsSnapshot, LuaBlockResult, LuaProgram,
    Request, ScriptReport, SectionVm, ToolBinding, ToolCallCounts, ToolCallOutcome, ToolOutputKind,
    ToolSet, YieldParse, current_tool_bindings, dispatch_tool, install_agent_chat_shim,
    install_runtime_events, resolve_model_binding,
};
use promptforge_model_client::client::{
    Completion, CompletionResult, GatewayClient, Message, StreamDelta, ToolSchema,
};
use promptforge_model_client::model::{
    ModelBinding, ModelCatalog, ModelInvocation, ModelSet, ModelView,
};
use promptforge_store::StoreRef;
use promptforge_tools::ToolCatalog;

use crate::config::AgentConfig;

/// A type-erased owned error cause.
type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

/// The reason one agent run failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentError {
    /// The host fired the run's cancel handle ([`AgentConfig::cancel`]).
    #[error("interrupted")]
    Interrupted,

    /// The agent program failed: a Lua compile or runtime error, an
    /// exhausted Lua resource quota, a failed host contract, or a dispatch
    /// failure the program did not catch.
    #[error("{message}")]
    Program {
        /// The failure rendered as its location-tagged diagnostic.
        message: String,
        /// The originating typed error, kept as the cause when one exists.
        #[source]
        source: Option<BoxedSource>,
    },

    /// A model call failed in transport or protocol terms.
    #[error("{message}")]
    Model {
        /// The completion failure's rendered message.
        message: String,
        /// The client's typed completion error, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// An internal runtime invariant was violated (a state the surrounding
    /// code has already guaranteed cannot occur).
    #[error("internal invariant violated: {0}")]
    Internal(&'static str),
}

/// Maps the Lua substrate onto the agent's public error. Cancellation stays
/// typed; the source-bearing variants keep their causes; everything else -
/// authoring errors, quotas, host-contract failures - degrades to its
/// display string under [`AgentError::Program`], because an agent run has
/// no binding phase and cannot reach the substrate's resolution variants.
impl From<LuaError> for AgentError {
    fn from(error: LuaError) -> AgentError {
        match error {
            LuaError::Interrupted => AgentError::Interrupted,
            LuaError::LuaRuntime { message, source } | LuaError::Tool { message, source } => {
                AgentError::Program {
                    message,
                    source: Some(source),
                }
            }
            LuaError::LuaCompile {
                location,
                source_line,
                message,
                source,
                ..
            } => AgentError::Program {
                message: format!(
                    "lua compilation error at {location} (line {source_line}): {message}"
                ),
                source: Some(source),
            },
            other => AgentError::Program {
                message: other.to_string(),
                source: None,
            },
        }
    }
}

/// Runs a `.lua` agent program in an agent VM.
///
/// Agent-only host calls: `models.chat(messages, opts)` - one stateless
/// tool-capable model round, streaming its deltas to
/// [`AgentConfig::on_delta`] - and `runtime.events()` - a read-only indexed
/// view over [`AgentConfig::event_log`] whose snapshot length bound
/// refreshes at every host-call resume - with `ui()` installed by a later
/// step. Shared kernel: `tool_call`, `store`, `var`, cancel checkpoints,
/// `models.infer`. `execute()`, `fanout()`, and `jump()` do not exist
/// here - absent, not stubbed. `run_agent` installs `config.cancel` as
/// the task's cancel scope, so every suspended host call races
/// cancellation through the shared dispatch.
///
/// Every tool in `tools` is registered by its wire name with no semantic
/// resolution; every model in `models` is addressable by its catalog name
/// through `models.use` and `models.get`, with no prompt-wide default. The
/// gateway client resolves lazily from the environment on first inference,
/// the same fallback core's scheduler applies when a run supplies no
/// client.
///
/// # Errors
/// Returns [`AgentError::Interrupted`] when `config.cancel` fires while the
/// program runs or a host call is suspended; [`AgentError::Program`] when
/// the program itself fails; [`AgentError::Model`] when a model call fails;
/// [`AgentError::Internal`] when a driver invariant is violated.
pub async fn run_agent(
    source: &str,
    tools: &ToolCatalog,
    models: &ModelCatalog,
    store: &StoreRef,
    config: AgentConfig,
) -> Result<(), AgentError> {
    run_agent_with_client(source, tools, models, store, config, None).await
}

/// [`run_agent`] with an explicit gateway client instead of the lazy
/// environment resolution.
///
/// This is the host-injection seam: a host that already knows its gateway
/// (the Workshop reads one from `workshop.toml`) passes the client it
/// built, so the agent's model calls reach the configured gateway rather
/// than whatever `PROMPTFORGE_GATEWAY_URL` names. `None` falls back to
/// the environment resolution, making this a strict superset of
/// [`run_agent`].
///
/// # Errors
/// Exactly [`run_agent`]'s: [`AgentError::Interrupted`] on a fired cancel
/// handle, [`AgentError::Program`] when the program fails,
/// [`AgentError::Model`] when a model call fails, and
/// [`AgentError::Internal`] on a violated driver invariant.
pub async fn run_agent_with_client(
    source: &str,
    tools: &ToolCatalog,
    models: &ModelCatalog,
    store: &StoreRef,
    config: AgentConfig,
    client: Option<GatewayClient>,
) -> Result<(), AgentError> {
    let cancel = config.cancel.clone();
    cancel::scope(cancel, drive(source, tools, models, store, config, client)).await
}

/// One agent run: compile, build the agent VM, drive the program coroutine
/// to its end, tear down. Runs inside the installed cancel scope.
async fn drive(
    source: &str,
    tools: &ToolCatalog,
    models: &ModelCatalog,
    store: &StoreRef,
    config: AgentConfig,
    client: Option<GatewayClient>,
) -> Result<(), AgentError> {
    let AgentConfig {
        name,
        execution,
        observer,
        event_log,
        on_delta,
        limits,
        ..
    } = config;
    let program = LuaProgram::compile(
        source,
        &format!("agent `{name}`"),
        NonZeroU32::MIN,
        &execution,
        observer.as_ref(),
        &name,
    )?;
    let tool_set = agent_tool_set(tools);
    let model_set = agent_model_set(models);
    let nonce = GuardNonce::fresh();
    let mut vm = SectionVm::new_for_section(
        &nonce,
        &tool_set,
        &model_set,
        &execution,
        observer.as_ref(),
        &name,
    )?;
    // A limits failure propagates bare, before any teardown observation
    // exists - the section drivers' contract.
    vm.apply_lua_limits(limits.lua_memory_bytes, limits.lua_log_events)?;
    let (counts, events) =
        match setup_agent_vm(&mut vm, store, &observer, &name, &tool_set, event_log) {
            Ok(installed) => installed,
            Err(error) => {
                vm.teardown(observer.as_ref(), &name);
                return Err(error);
            }
        };
    let run = AgentRun {
        vm: &vm,
        program: &program,
        tool_set: &tool_set,
        model_view: Mutex::new(model_set),
        counts,
        events,
        nonce: &nonce,
        observer: &observer,
        execution: &execution,
        name: &name,
        turns: AtomicU32::new(0),
        client: Mutex::new(client),
        on_delta,
    };
    // The whole agent program is one chunk; the driver owns its observation
    // boundaries, exactly as core's scheduler owns a block's.
    observer.observe(&execution, &name, detail::LUA_CHUNK_STARTED);
    let result = drive_program(&run).await;
    observer.observe(
        &execution,
        &name,
        if result.is_ok() {
            detail::LUA_CHUNK_SUCCEEDED
        } else {
            detail::LUA_CHUNK_FAILED
        },
    );
    vm.teardown(observer.as_ref(), &name);
    result
}

/// Registers every catalog tool by its wire name, with no semantic
/// resolution: one binding per tool, every alias in scope (the
/// `always` list), so `tool_call` reaches the whole catalog. Wire names are
/// assumed unique within one agent catalog; on a collision the first
/// binding wins alias lookup. A tool declaring
/// [`structured_output`](promptforge_tools::Tool::structured_output) binds
/// with the structured output kind, so its JSON output resumes into the
/// program as a table; everything else stays plain text.
fn agent_tool_set(catalog: &ToolCatalog) -> ToolSet {
    let bindings: Vec<ToolBinding> = catalog
        .tools()
        .iter()
        .map(|tool| ToolBinding {
            alias: tool.wire_name().to_owned(),
            description: tool.description().to_owned(),
            id: tool.id(),
            model_description: None,
            tool: Arc::clone(tool),
            conflicts: Vec::new(),
            output_kind: if tool.structured_output() {
                ToolOutputKind::Structured
            } else {
                ToolOutputKind::Plain
            },
        })
        .collect();
    let always = bindings
        .iter()
        .map(|binding| binding.alias.clone())
        .collect();
    ToolSet::from_parts(bindings, always)
}

/// Registers every catalog model by its catalog name: one binding per
/// descriptor with the default invocation (no temperature, token, or
/// thinking overrides) and no prompt-wide default, so a bare `models.infer`
/// requires a prior `models.use` selection.
fn agent_model_set(catalog: &ModelCatalog) -> ModelSet {
    ModelSet {
        bindings: catalog
            .models()
            .iter()
            .map(|descriptor| {
                ModelBinding::new(
                    descriptor.id().name(),
                    descriptor.description(),
                    descriptor.id().clone(),
                    ModelInvocation {
                        temperature: None,
                        max_tokens: None,
                        thinking: None,
                    },
                    descriptor.context(),
                )
            })
            .collect(),
        default: None,
    }
}

/// The agent VM's setup sequence: the section construction reused (host
/// injection, host APIs, the coroutine shims) minus the section control
/// surface, plus the agent-only installs - the `models.chat` shim, the
/// read-only `tools.calls` counter surface over the run's dispatch counts,
/// and the `runtime.events()` view over the host's [`EventLog`]. Returns
/// the counts the dispatches increment and, when a log was supplied, the
/// driver's [`EventsSnapshot`] refresh handle.
///
/// Absent, not stubbed: the shared shim prelude installs `execute` and
/// `fanout` for section VMs, but the agent kernel is `models.infer` and
/// `tool_call` alone, so both globals are removed here, before any author
/// code runs - an agent touching them fails as an undefined global. `jump`
/// is never installed at all: the scheduler control-global install is
/// skipped outright. `models.chat` is the mirror image: installed here and
/// never in a section VM.
fn setup_agent_vm(
    vm: &mut SectionVm,
    store: &StoreRef,
    observer: &Arc<dyn Observer>,
    name: &str,
    tool_set: &ToolSet,
    event_log: Option<Arc<dyn EventLog>>,
) -> Result<(ToolCallCounts, Option<EventsSnapshot>), AgentError> {
    vm.inject_host_with_var("", &serde_json::json!({}), store, None, None, None)?;
    vm.install_host_apis(observer, name)?;
    vm.install_coro_shims()?;
    install_agent_chat_shim(vm.lua())?;
    let counts = vm.install_tool_call_counts(tool_set.bindings())?;
    let events = install_runtime_events(vm.lua(), event_log)?;
    let globals = vm.lua().globals();
    for global in ["execute", "fanout"] {
        globals
            .raw_set(global, mlua::Value::Nil)
            .map_err(|error| AgentError::Program {
                message: format!("removing the `{global}` shim from the agent VM failed"),
                source: Some(Box::new(error)),
            })?;
    }
    Ok((counts, events))
}

/// The borrowed run pieces every driver step reads.
struct AgentRun<'a> {
    /// The agent VM the program coroutine runs on.
    vm: &'a SectionVm,
    /// The compiled agent program.
    program: &'a LuaProgram,
    /// The frozen tool bindings (`tool_call`'s scope).
    tool_set: &'a ToolSet,
    /// The frozen model bindings behind `models.use`/`models.get`, read
    /// through the `ModelView` impl on the mutex.
    model_view: Mutex<ModelSet>,
    /// Per-alias dispatch counts, seeded with every catalog alias and read
    /// back by the program through the `tools.calls` table.
    counts: ToolCallCounts,
    /// The driver side of the `runtime.events()` view, when the host
    /// supplied an [`EventLog`]: its length bound is refreshed at every
    /// host-call resume.
    events: Option<EventsSnapshot>,
    /// The run's untrusted-wrap nonce.
    nonce: &'a GuardNonce,
    /// The run's reporting sink.
    observer: &'a Arc<dyn Observer>,
    /// The run's execution id.
    execution: &'a str,
    /// The agent's name, every observer call's `section` label.
    name: &'a str,
    /// Completed model turns, reported on tool dispatches.
    turns: AtomicU32,
    /// The gateway client slot: the injected client, else resolved once
    /// from the environment on first inference. Locked briefly and never
    /// across an await.
    client: Mutex<Option<GatewayClient>>,
    /// The host's live streaming-delta callback; `models.chat` forwards
    /// every [`StreamDelta`] to it. Deltas never ride the observer.
    on_delta: Option<Arc<dyn Fn(StreamDelta) + Send + Sync>>,
}

impl AgentRun<'_> {
    /// The run's gateway client: the slot's, resolved once from the
    /// environment when the caller injected none - the same lazy fallback
    /// core's scheduler applies.
    fn client(&self) -> Result<GatewayClient, AgentError> {
        let mut slot = self
            .client
            .lock()
            .map_err(|_| AgentError::Internal("the agent client slot was poisoned"))?;
        if let Some(client) = slot.as_ref() {
            return Ok(client.clone());
        }
        let client = GatewayClient::from_env().map_err(|error| AgentError::Model {
            message: error.to_string(),
            source: Box::new(error),
        })?;
        *slot = Some(client.clone());
        Ok(client)
    }

    /// Applies the resume-refresh rule: republishes the events snapshot's
    /// length bound, so appends that landed while the program was
    /// suspended - or synchronously from a host callback while it ran -
    /// become visible exactly at the resume this call precedes.
    fn refresh_events(&self) {
        if let Some(events) = &self.events {
            events.refresh();
        }
    }
}

/// Drives the program coroutine to its end: resume, validate the yield,
/// dispatch the one in-flight request, resume with the answer.
async fn drive_program(run: &AgentRun<'_>) -> Result<(), AgentError> {
    let mut step = run.vm.start_block_coro(run.program)?;
    loop {
        match step {
            // The program returned: the run is complete. A scalar return
            // value has no consumer at this step; completion is the signal.
            CoroStep::Done(LuaBlockResult::Returned(_)) => return Ok(()),
            CoroStep::Done(LuaBlockResult::Jump(_)) => {
                // Unreachable: the jump global is never installed in an
                // agent VM, so no chunk can record a transfer.
                return Err(AgentError::Internal(
                    "an agent VM cannot record a jump: the jump global is never installed",
                ));
            }
            CoroStep::Yielded(thread, values) => {
                step = match run.vm.request_from_yield(&values) {
                    YieldParse::Request(request) => {
                        let answer = dispatch(run, request).await?;
                        run.refresh_events();
                        run.vm
                            .resume_block_coro_answer(run.program, &thread, answer)?
                    }
                    // An argument-validation failure is the call's answer:
                    // the shim raises it at the call site, so an author
                    // `pcall` catches it.
                    YieldParse::Call(answer) => {
                        run.refresh_events();
                        run.vm.resume_block_coro_answer(
                            run.program,
                            &thread,
                            answer.map_error(AgentError::from),
                        )?
                    }
                    YieldParse::Malformed(error) => return Err(error.into()),
                };
            }
        }
    }
}

/// Dispatches one validated request: the leaf calls the kernel installs,
/// plus unreachable internal-invariant guards for the section-only
/// requests, mirroring core's guard for the agent-only ones.
///
/// A dispatch failure rides back as the call's answer so the program can
/// `pcall` it; cancellation alone fails the run instead of resuming.
async fn dispatch(run: &AgentRun<'_>, request: Request) -> Result<Answer<AgentError>, AgentError> {
    match request {
        Request::Infer { prompt, binding } => match dispatch_infer(run, &prompt, binding).await {
            Err(AgentError::Interrupted) => Err(AgentError::Interrupted),
            outcome => Ok(Answer::Infer(outcome)),
        },
        Request::ToolCall { alias, args } => match dispatch_tool_call(run, &alias, args).await {
            Err(AgentError::Interrupted) => Err(AgentError::Interrupted),
            outcome => Ok(Answer::ToolCallResult(outcome)),
        },
        Request::Chat {
            messages,
            model,
            tools,
        } => match dispatch_chat(run, &messages, model, &tools).await {
            Err(AgentError::Interrupted) => Err(AgentError::Interrupted),
            outcome => Ok(Answer::Chat(outcome.map(Box::new))),
        },
        // Unreachable: the execute/fanout shims are removed from the agent
        // VM before author code runs, no shim produces an mcp request, and
        // stripped coroutines make a hand-rolled yield fail validation
        // before dispatch.
        Request::Execute { .. } => Err(AgentError::Internal(
            "an agent VM cannot yield an execute request: the shim is never installed",
        )),
        Request::Fanout { .. } => Err(AgentError::Internal(
            "an agent VM cannot yield a fanout request: the shim is never installed",
        )),
        Request::Mcp { .. } => Err(AgentError::Internal(
            "an agent VM cannot yield an mcp request: no shim produces one",
        )),
    }
}

/// One `models.infer` round: the handle's frozen binding or the program's
/// `models.use` selection, one direct tool-free gateway call on a fresh
/// conversation, raced against cancellation. Reported like a section's
/// infer round; an aborted round reports nothing, matching the scheduler's
/// abort path.
async fn dispatch_infer(
    run: &AgentRun<'_>,
    prompt: &str,
    binding: Option<ModelBinding>,
) -> Result<String, AgentError> {
    let binding = match binding {
        Some(binding) => binding,
        None => {
            resolve_model_binding(&run.model_view, &run.vm.model_runtime)?.ok_or_else(|| {
                AgentError::Program {
                    message: "no model is selected: call models.use(...) before models.infer"
                        .to_owned(),
                    source: None,
                }
            })?
        }
    };
    let client = run.client()?;
    let options = binding.completion_options();
    let conversation = [Message::user(prompt)];
    // The one future the driver awaits, raced against the installed cancel
    // scope so a suspended infer cannot hold the run past a cancel. A
    // nested infer round consumes only the accumulated completion; live
    // deltas have no consumer here.
    let completion = tokio::select! {
        biased;
        () = cancel::wait_cancelled() => return Err(AgentError::Interrupted),
        completion = client.complete(&conversation, None, &options, |_| {}) => completion,
    };
    let completion = match completion {
        Ok(completion) => completion,
        Err(error) => {
            run.observer
                .observe(run.execution, run.name, detail::MODEL_TURN_FAILED);
            return Err(AgentError::Model {
                message: error.to_string(),
                source: Box::new(error),
            });
        }
    };
    run.turns.fetch_add(1, Ordering::Relaxed);
    run.observer
        .observe(run.execution, run.name, detail::MODEL_TURN_COMPLETED);
    match completion.result {
        CompletionResult::Text(text) => {
            if completion.finish_reason.as_deref() == Some("length") {
                run.observer
                    .observe(run.execution, run.name, detail::MODEL_TURN_TRUNCATED);
            }
            Ok(text)
        }
        // No tools were advertised, so a tool-call turn is a backend
        // protocol violation rather than something to dispatch.
        CompletionResult::ToolCalls(_) => Err(AgentError::Program {
            message: "model inference received tool calls but no tools were advertised".to_owned(),
            source: None,
        }),
        // `CompletionResult` is `#[non_exhaustive]` across the crate seam:
        // an unrecognized future outcome is the same violation.
        _ => Err(AgentError::Program {
            message:
                "model inference received an unrecognized outcome but no tools were advertised"
                    .to_owned(),
            source: None,
        }),
    }
}

/// One `models.chat` round: one stateless tool-capable gateway call over
/// the program-built message list, streaming deltas to the host's
/// callback, raced against cancellation.
///
/// The binding is `opts.model` (a catalog model name) or the program's
/// `models.use` selection; the advertised tools are exactly `opts.tools`
/// (default none - the driver adds nothing, so a host-primitive tool is
/// never advertised). A completed round fires `on_thinking` when the model
/// thought, then `on_assistant_reply` or `on_assistant_tool_calls`, each
/// with model and metrics; requested tool calls resume unexecuted -
/// dispatching them is the program's decision, taken on their presence,
/// never on `finish_reason`. The model client fails the batch when
/// `length` or `content_filter` truncates a tool-call round, and that
/// failure rides back as this call's answer.
async fn dispatch_chat(
    run: &AgentRun<'_>,
    messages: &serde_json::Value,
    model: Option<String>,
    tools: &[String],
) -> Result<ChatResult, AgentError> {
    let binding = match model {
        Some(name) => ModelView::binding(&run.model_view, &name)
            .map_err(|error| AgentError::Program {
                message: error.to_string(),
                source: Some(Box::new(error)),
            })?
            .ok_or_else(|| AgentError::Program {
                message: format!("model {name:?} is not in this agent's catalog"),
                source: None,
            })?,
        None => {
            resolve_model_binding(&run.model_view, &run.vm.model_runtime)?.ok_or_else(|| {
                AgentError::Program {
                    message: "no model is selected: pass opts.model or call models.use(...) \
                              before models.chat"
                        .to_owned(),
                    source: None,
                }
            })?
        }
    };
    let schemas = advertised_schemas(run, tools)?;
    let conversation = wire_messages(messages)?;
    let client = run.client()?;
    let options = binding.completion_options();
    let tool_arg = if schemas.is_empty() {
        None
    } else {
        Some(schemas.as_slice())
    };
    // The one future the driver awaits, raced against the installed cancel
    // scope. Deltas forward live to the host's callback as they stream;
    // they never enter the observer.
    let completion = tokio::select! {
        biased;
        () = cancel::wait_cancelled() => return Err(AgentError::Interrupted),
        completion = client.complete(&conversation, tool_arg, &options, |delta| {
            if let Some(on_delta) = &run.on_delta {
                on_delta(delta);
            }
        }) => completion,
    };
    let completion = match completion {
        Ok(completion) => completion,
        Err(error) => {
            run.observer
                .observe(run.execution, run.name, detail::MODEL_TURN_FAILED);
            return Err(AgentError::Model {
                message: error.to_string(),
                source: Box::new(error),
            });
        }
    };
    chat_round_result(run, completion)
}

/// Applies one completed chat round: counts the turn, reports the
/// operational boundary, fires the content events - thinking first, then
/// the reply or the unexecuted tool-call batch, each with model and
/// metrics - and shapes the [`ChatResult`] the program resumes with.
fn chat_round_result(run: &AgentRun<'_>, completion: Completion) -> Result<ChatResult, AgentError> {
    let turn = run.turns.fetch_add(1, Ordering::Relaxed) + 1;
    run.observer
        .observe(run.execution, run.name, detail::MODEL_TURN_COMPLETED);
    let metrics = call_metrics(&completion);
    let model_name = completion.model().to_owned();
    if let Some(thinking) = completion
        .reasoning_content()
        .filter(|text| !text.is_empty())
    {
        run.observer
            .on_thinking(run.execution, run.name, 0, 0, turn, &model_name, thinking);
    }
    let finish_reason = completion.finish_reason().map(str::to_owned);
    match completion.result {
        CompletionResult::Text(text) => {
            if finish_reason.as_deref() == Some("length") {
                run.observer
                    .observe(run.execution, run.name, detail::MODEL_TURN_TRUNCATED);
            }
            run.observer.on_assistant_reply(
                run.execution,
                run.name,
                0,
                0,
                turn,
                &text,
                finish_reason.as_deref(),
                &model_name,
                metrics.as_ref(),
            );
            Ok(ChatResult {
                reply: Some(text),
                tool_calls: None,
                finish_reason,
                model: model_name,
                metrics,
            })
        }
        CompletionResult::ToolCalls(calls) => {
            let events: Vec<ToolCallEvent> = calls
                .into_iter()
                .map(|call| ToolCallEvent {
                    id: call.id,
                    name: call.name,
                    arguments: call.arguments,
                })
                .collect();
            run.observer.on_assistant_tool_calls(
                run.execution,
                run.name,
                0,
                0,
                turn,
                &model_name,
                &events,
            );
            Ok(ChatResult {
                reply: None,
                tool_calls: Some(events),
                finish_reason,
                model: model_name,
                metrics,
            })
        }
        // `CompletionResult` is `#[non_exhaustive]` across the crate seam:
        // an unrecognized future outcome cannot be resumed into the program.
        _ => Err(AgentError::Program {
            message: "model chat received an unrecognized completion outcome".to_owned(),
            source: None,
        }),
    }
}

/// Builds the advertised tool schemas for one chat round: exactly the
/// `opts.tools` aliases, resolved against the agent's effective scope, in
/// the author's order. The driver never adds to the set.
fn advertised_schemas(run: &AgentRun<'_>, tools: &[String]) -> Result<Vec<ToolSchema>, AgentError> {
    if tools.is_empty() {
        return Ok(Vec::new());
    }
    let effective = current_tool_bindings(run.tool_set, &run.vm.tool_runtime)?;
    let mut schemas = Vec::with_capacity(tools.len());
    for alias in tools {
        let Some(binding) = effective.iter().find(|binding| binding.alias() == alias) else {
            let in_scope: Vec<&str> = effective.iter().map(ToolBinding::alias).collect();
            return Err(AgentError::Program {
                message: format!(
                    "tool alias {alias:?} is not registered with this agent; in scope: {in_scope:?}"
                ),
                source: None,
            });
        };
        let description = binding
            .model_description()
            .unwrap_or_else(|| binding.tool().description())
            .to_owned();
        let schema = ToolSchema::new(
            binding.alias().to_owned(),
            description,
            binding.tool().parameters_schema(),
        )
        .map_err(|error| AgentError::Program {
            message: format!("tool alias {alias:?} cannot be advertised to the model"),
            source: Some(Box::new(error)),
        })?;
        schemas.push(schema);
    }
    Ok(schemas)
}

/// Converts the protocol-validated message array into the client's wire
/// messages. The protocol parse validated the shape once - roles, content,
/// tool ids - so a violation here is a driver invariant failure, never an
/// author-facing error, and no second validator exists to drift. Each
/// entry contributes exactly the four fields the wire message carries
/// (`role`, `content`, `tool_call_id`, `tool_calls`); other entry fields
/// are dropped.
fn wire_messages(messages: &serde_json::Value) -> Result<Vec<Message>, AgentError> {
    const VALIDATED: &str = "a chat request reaching dispatch carries protocol-validated messages";
    let entries = messages.as_array().ok_or(AgentError::Internal(VALIDATED))?;
    entries
        .iter()
        .map(|entry| {
            let role = entry
                .get("role")
                .and_then(serde_json::Value::as_str)
                .ok_or(AgentError::Internal(VALIDATED))?;
            let content = entry
                .get("content")
                .cloned()
                .ok_or(AgentError::Internal(VALIDATED))?;
            let tool_call_id = entry
                .get("tool_call_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            let tool_calls = entry
                .get("tool_calls")
                .and_then(serde_json::Value::as_array)
                .cloned();
            Ok(Message::from_validated_parts(
                role,
                content,
                tool_call_id,
                tool_calls,
            ))
        })
        .collect()
}

/// Assembles the round's [`CallMetrics`] from everything the completion
/// measured, or `None` when nothing was measured.
fn call_metrics(completion: &Completion) -> Option<CallMetrics> {
    let metrics = CallMetrics {
        usage: completion.usage().cloned(),
        llama: completion.llama_timings().cloned(),
        vllm: completion.vllm_metrics().cloned(),
        client: completion.client_timing().cloned(),
    };
    let measured = metrics.usage.is_some()
        || metrics.llama.is_some()
        || metrics.vllm.is_some()
        || metrics.client.is_some();
    measured.then_some(metrics)
}

/// One `tool_call` dispatch: the alias resolved against the agent's
/// registered catalog, then the shared [`dispatch_tool`] body (cancel race,
/// counts, untrusted wrap, observer events), classified by the binding's
/// declared output kind.
async fn dispatch_tool_call(
    run: &AgentRun<'_>,
    alias: &str,
    args: serde_json::Value,
) -> Result<ToolCallOutcome, AgentError> {
    let effective = current_tool_bindings(run.tool_set, &run.vm.tool_runtime)?;
    let Some(binding) = effective
        .iter()
        .find(|binding| binding.alias() == alias)
        .cloned()
    else {
        let in_scope: Vec<&str> = effective.iter().map(ToolBinding::alias).collect();
        return Err(AgentError::Program {
            message: format!(
                "tool alias {alias:?} is not registered with this agent; in scope: {in_scope:?}"
            ),
            source: None,
        });
    };
    // Agents have no fanout chains or execute nesting: chain 0, depth 0.
    let report = ScriptReport {
        chain_id: 0,
        depth: 0,
        turn: run.turns.load(Ordering::Relaxed),
    };
    let text = dispatch_tool(
        &binding,
        args,
        Some(&run.counts),
        run.nonce,
        run.observer.as_ref(),
        run.execution,
        run.name,
        Some(report),
    )
    .await?;
    ToolCallOutcome::from_dispatch(binding.output_kind, binding.alias(), text)
        .map_err(AgentError::from)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use promptforge_core_support::cancel::CancelHandle;
    use promptforge_core_support::observe::NullObserver;
    use promptforge_model_client::client::{GatewayEndpoint, SecretString};
    use promptforge_model_client::model::{ModelDescriptor, ModelId, ThinkingMode};

    use super::*;
    use crate::config::AgentLimits;

    const EXECUTION: &str = "agent-test";

    fn config() -> AgentConfig {
        AgentConfig {
            name: "test-agent".to_owned(),
            execution: EXECUTION.to_owned(),
            observer: Arc::new(NullObserver::default()),
            cancel: CancelHandle::new(),
            event_log: None,
            on_delta: None,
            ui: None,
            limits: AgentLimits::default(),
        }
    }

    fn empty_tools() -> ToolCatalog {
        ToolCatalog::new(&[]).expect("an empty catalog is valid")
    }

    #[tokio::test]
    async fn a_trivial_agent_writes_to_the_store_and_returns() {
        let store = StoreRef::memory();
        run_agent(
            "store.write('notes.txt', 'from the agent')\nreturn 'done'",
            &empty_tools(),
            &ModelCatalog::empty(),
            &store,
            config(),
        )
        .await
        .expect("the trivial agent runs to completion");
        assert_eq!(
            store.read("notes.txt").expect("the agent's write persists"),
            "from the agent",
            "the agent's store write must be visible through the run-scoped handle"
        );
    }

    #[tokio::test]
    async fn the_control_globals_are_nil_in_the_agent_vm() {
        // Absent, not stubbed: a stub function would tostring as
        // `function: 0x...`; only true absence renders three nils.
        let store = StoreRef::memory();
        let error = run_agent(
            "return tostring(execute) .. ' ' .. tostring(fanout) .. ' ' .. tostring(jump)",
            &empty_tools(),
            &ModelCatalog::empty(),
            &store,
            config(),
        )
        .await;
        assert!(
            error.is_ok(),
            "reading the absent globals is not an error: {error:?}"
        );
        // The scalar return is not surfaced by run_agent; prove nil-ness
        // through the store instead.
        run_agent(
            "store.write('nils.txt', tostring(execute) .. ' ' .. tostring(fanout) .. ' ' .. tostring(jump))",
            &empty_tools(),
            &ModelCatalog::empty(),
            &store,
            config(),
        )
        .await
        .expect("the probe agent runs");
        assert_eq!(
            store.read("nils.txt").expect("the probe wrote its reading"),
            "nil nil nil",
            "execute, fanout, and jump must all be nil in the agent VM"
        );
    }

    #[tokio::test]
    async fn calling_an_absent_control_global_is_an_undefined_global_failure() {
        for global in ["execute", "fanout", "jump"] {
            let store = StoreRef::memory();
            let source = format!("{global}('anything')");
            let error = run_agent(
                &source,
                &empty_tools(),
                &ModelCatalog::empty(),
                &store,
                config(),
            )
            .await
            .expect_err("calling an absent control global must fail the run");
            let message = error.to_string();
            assert!(
                message.contains("attempt to call a nil value") && message.contains(global),
                "`{global}` must fail as an undefined global, got: {message}"
            );
            assert!(
                matches!(error, AgentError::Program { .. }),
                "an absent-global failure is a plain program error, never a typed variant: {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn firing_cancel_interrupts_a_suspended_models_infer() {
        // A gateway that accepts the connection and never answers, so only
        // cancellation can end the round.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral listener binds");
        let addr = listener.local_addr().expect("the listener has an address");
        let endpoint = GatewayEndpoint::new(&format!("http://{addr}/v1"))
            .expect("the test endpoint is a valid URL");
        let key = SecretString::new("test-key").expect("the test key is non-empty");
        let client = GatewayClient::new(endpoint, key);
        let cancel = CancelHandle::new();
        let fire = cancel.clone();
        let mut run_config = config();
        run_config.cancel = cancel;
        let context = NonZeroU32::new(4096).expect("4096 is non-zero");
        let models = ModelCatalog::new([ModelDescriptor::new(
            ModelId::gateway("test-model").expect("the test model name is valid"),
            "a test model",
            context,
            ThinkingMode::Never,
        )])
        .expect("the test catalog has one unique model");
        let run = tokio::spawn(async move {
            let store = StoreRef::memory();
            run_agent_with_client(
                "models.use('test-model')\nreturn models.infer('hello')",
                &empty_tools(),
                &models,
                &store,
                run_config,
                Some(client),
            )
            .await
        });
        // The agent is suspended on models.infer once its request connects;
        // the accepted socket is held open unanswered until the cancel
        // fires, so the round cannot end any other way.
        let (_socket, _) = listener.accept().await.expect("the infer request connects");
        fire.cancel();
        let result = run.await.expect("the run task joins");
        assert!(
            matches!(result, Err(AgentError::Interrupted)),
            "a cancelled suspended infer must interrupt the run, got {result:?}"
        );
    }
}
