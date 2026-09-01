//! `run_agent` and its leaf-dispatch driver.
//!
//! An agent program is one Lua chunk run as one coroutine on an agent VM -
//! a [`SectionVm`] built with the section construction sequence (harden,
//! untrusted, host injection, store, log, var) minus the section control
//! surface. The shared kernel is `models.infer` and `tool_call`; `execute`,
//! `fanout`, and `jump` are absent, not stubbed, so touching them is an
//! undefined-global failure. The driver is leaf dispatch only: it resumes
//! the coroutine, validates each yield into a [`Request`], awaits exactly
//! one future - the current request - and resumes with the answer. Tool
//! dispatch goes through the shared [`dispatch_tool`] body; nothing here
//! duplicates it.
//!
//! `run_agent` installs [`AgentConfig::cancel`] as the task's cancel scope:
//! suspended host calls race cancellation, and running Lua observes the
//! same flag through the VM's instruction hook. Teardown is observed like a
//! section's, under the agent's name as the `section` label.

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use promptforge_core_support::cancel;
use promptforge_core_support::observe::{Observer, detail};
use promptforge_core_support::untrusted::GuardNonce;
use promptforge_lua::{
    Answer, CoroStep, Error as LuaError, LuaBlockResult, LuaProgram, Request, ScriptReport,
    SectionVm, ToolBinding, ToolCallCounts, ToolCallOutcome, ToolOutputKind, ToolSet, YieldParse,
    current_tool_bindings, dispatch_tool, resolve_model_binding,
};
use promptforge_model_client::client::{CompletionResult, GatewayClient, Message};
use promptforge_model_client::model::{ModelBinding, ModelCatalog, ModelInvocation, ModelSet};
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
/// Agent-only host calls (`models.chat`, `runtime.events()`, `ui()`) are
/// installed by later steps. Shared kernel: `tool_call`, `store`, `var`,
/// cancel checkpoints, `models.infer`. `execute()`, `fanout()`, and
/// `jump()` do not exist here - absent, not stubbed. `run_agent` installs
/// `config.cancel` as the task's cancel scope, so every suspended host call
/// races cancellation through the shared dispatch.
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
/// environment resolution: the crate's own test seam.
pub(crate) async fn run_agent_with_client(
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
    if let Err(error) = setup_agent_vm(&mut vm, store, &observer, &name) {
        vm.teardown(observer.as_ref(), &name);
        return Err(error);
    }
    let run = AgentRun {
        vm: &vm,
        program: &program,
        tool_set: &tool_set,
        model_view: Mutex::new(model_set),
        counts: ToolCallCounts::new(tool_set.bindings().iter().map(|b| b.alias().to_owned())),
        nonce: &nonce,
        observer: &observer,
        execution: &execution,
        name: &name,
        turns: AtomicU32::new(0),
        client: Mutex::new(client),
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
/// resolution: one plain binding per tool, every alias in scope (the
/// `always` list), so `tool_call` reaches the whole catalog. Wire names are
/// assumed unique within one agent catalog; on a collision the first
/// binding wins alias lookup.
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
            output_kind: ToolOutputKind::Plain,
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
/// surface.
///
/// Absent, not stubbed: the shared shim prelude installs `execute` and
/// `fanout` for section VMs, but the agent kernel is `models.infer` and
/// `tool_call` alone, so both globals are removed here, before any author
/// code runs - an agent touching them fails as an undefined global. `jump`
/// is never installed at all: the scheduler control-global install is
/// skipped outright.
fn setup_agent_vm(
    vm: &mut SectionVm,
    store: &StoreRef,
    observer: &Arc<dyn Observer>,
    name: &str,
) -> Result<(), AgentError> {
    vm.inject_host_with_var("", &serde_json::json!({}), store, None, None, None)?;
    vm.install_host_apis(observer, name)?;
    vm.install_coro_shims()?;
    let globals = vm.lua().globals();
    for global in ["execute", "fanout"] {
        globals
            .raw_set(global, mlua::Value::Nil)
            .map_err(|error| AgentError::Program {
                message: format!("removing the `{global}` shim from the agent VM failed"),
                source: Some(Box::new(error)),
            })?;
    }
    Ok(())
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
    /// Per-alias dispatch counts, seeded with every catalog alias.
    counts: ToolCallCounts,
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
                        run.vm
                            .resume_block_coro_answer(run.program, &thread, answer)?
                    }
                    // An argument-validation failure is the call's answer:
                    // the shim raises it at the call site, so an author
                    // `pcall` catches it.
                    YieldParse::Call(answer) => run.vm.resume_block_coro_answer(
                        run.program,
                        &thread,
                        answer.map_error(AgentError::from),
                    )?,
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
