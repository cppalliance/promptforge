//! Section lifecycle execution and fall-through.
//!
//! The run walks top-level sections in file order, creating one isolated
//! [`crate::lua::SectionVm`] for each. Shared Lua loads before host injection,
//! then the preamble, model reply binding, and epilog use that same VM. A
//! scalar preamble return skips the model and epilog; a scalar epilog return
//! ends the run after the model.
//!
//! Running off the last section ends the run: the result is `default_return`
//! from the frontmatter, else the last model reply, else a generic completion.
//!
//! One run-scoped [`StoreRef`] is created once by the caller and threaded through
//! every section (both its Lua preamble and, later, the model's file tools), so
//! bulk state persists across the context-clearing transitions even though a
//! section's conversation never does.
//!
//! A run reports itself as it goes: [`RunOptions::observer`] receives a
//! borrowed `(execution, section, detail)` record when the run starts and ends, at each
//! section boundary, model turn, tool call, and harness-mediated store
//! operation. Reporting is a side channel and never
//! a decision, so passing [`crate::observe::NullObserver`] changes nothing but
//! the silence.
//!
//! Bound tool declarations replay into each section VM. Prompt-wide aliases
//! and H2 additions form the effective model-visible scope, which is checked
//! for semantic near-duplicates before concrete tools are advertised under
//! their local aliases and dispatched by stable identity.
//!
//! Still to come: the other exit cases (a descriptor = goto/task/fanout).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::json;

use crate::bind::BoundPrompt;
use crate::client::{CompletionResult, GatewayClient, Message, ToolSchema};
use crate::debug::{DebugCapture, DebugEvent};
use crate::fanout;
use crate::lua::{SectionVm, ToolBindings, ToolScope};
use crate::model::{CompletionOptions, ModelBindings};
use crate::observe::{Observer, detail};
use crate::parser::Prompt;
use crate::store::StoreRef;
use crate::subst;
use crate::tools::{Tool, ToolId, ToolRegistry};
use crate::untrusted;
use crate::{Error, NearDuplicateDiagnostic, Result};

/// The default maximum number of model round trips a single section's
/// tool-call loop will take before giving up, applied when a prompt's
/// frontmatter does not declare its own `max_tool_iterations`.
const DEFAULT_MAX_TOOL_ITERATIONS: usize = 24;

/// The prompt language major this executor implements.
const SUPPORTED_MAJOR: u32 = 1;

/// Everything a run needs beyond the prompt, its input, its tools, and its
/// store: where progress is reported, and which gateway it talks to.
///
/// # Examples
/// ```
/// use promptforge_core::execute::RunOptions;
/// use promptforge_core::observe::NullObserver;
///
/// let opts = RunOptions {
///     execution: "example-run",
///     observer: &NullObserver,
///     client: None,
///     debug: None,
/// };
/// assert!(opts.client.is_none());
/// assert!(opts.debug.is_none());
/// ```
pub struct RunOptions<'a> {
    /// The caller-provided identifier shared by every report in this execution.
    pub execution: &'a str,
    /// Where the run reports its progress. Pass
    /// [`NullObserver`](crate::observe::NullObserver) to discard it.
    pub observer: &'a dyn Observer,
    /// The gateway client the run's model calls go through. `None` builds one
    /// from the process environment on the first call that needs it, which is
    /// what the CLI uses; a caller configured from a file (rather than from the
    /// environment) passes its own.
    pub client: Option<GatewayClient>,
    /// Opt-in raw request/response capture for model turns. `None` (the
    /// production default) skips the seam entirely so hosts pay nothing.
    pub debug: Option<&'a dyn DebugCapture>,
}

/// A prompt accepted by [`run`].
///
/// A [`BoundPrompt`] supplies frozen H1 declaration replay data. A parsed
/// [`Prompt`] remains accepted as a temporary input for tool-free hosts that
/// have not yet adopted the separate binding phase; it executes on the same
/// validated path as a bound prompt with empty frozen bindings, so any
/// `tools.need` or `tools.add` in it fails loudly, and its shared Lua runs
/// under full replay rules, where a scalar return is an error.
#[derive(Debug, Clone, Copy)]
pub struct RunPrompt<'a> {
    prompt: &'a Prompt,
    bound: Option<&'a BoundPrompt>,
}

impl<'a> From<&'a BoundPrompt> for RunPrompt<'a> {
    fn from(bound: &'a BoundPrompt) -> Self {
        Self {
            prompt: bound.prompt(),
            bound: Some(bound),
        }
    }
}

impl<'a> From<&'a Prompt> for RunPrompt<'a> {
    fn from(prompt: &'a Prompt) -> Self {
        Self {
            prompt,
            bound: None,
        }
    }
}

impl fmt::Debug for RunOptions<'_> {
    /// Formats the options without the observer or debug capture, which are
    /// trait objects and carry no `Debug`; their presence is reported instead.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunOptions")
            .field("execution", &self.execution)
            .field("observer", &"<dyn Observer>")
            .field("client", &self.client)
            .field(
                "debug",
                &if self.debug.is_some() {
                    "<dyn DebugCapture>"
                } else {
                    "None"
                },
            )
            .finish()
    }
}

/// Execute a prompt and return the run's result.
///
/// `args` is the single raw input string, exposed to Lua and to `{{ args }}`.
///
/// `prompt` normally receives a [`BoundPrompt`], whose frozen H1 declarations
/// replay in each section VM. Parsed [`Prompt`] values remain accepted so
/// existing tool-free hosts keep compiling until they adopt the separate
/// binding phase; they run through the same validated VM with empty frozen
/// bindings, so declaring or scoping any tool fails loudly and shared Lua
/// replays under the same rules as a bound prompt.
///
/// `tools` is the complete callable pool for this run. A bound section exposes
/// only aliases in its effective `tools.always` plus `tools.add` scope, and
/// dispatches a returned alias through its frozen [`crate::tools::ToolId`].
///
/// `store` is the run's virtual-file handle. Create it once (typically with
/// [`StoreRef::memory`]) and pass it in; the same handle is given to every
/// section's Lua preamble, so files persist across sections even though each
/// section's context is cleared on entry. It is a shared handle, so passing
/// `&store` (not a fresh store per section) is what makes the state durable.
///
/// `opts` carries the run's [`Observer`] and, optionally, the
/// [`GatewayClient`] to use. A run that clears the version gate reports
/// [`detail::RUN_STARTED`] before its first section and either
/// [`detail::RUN_SUCCEEDED`] or [`detail::RUN_FAILED`] however it ends. A run
/// refused by the gate reports nothing because it never started.
///
/// # Errors
/// Returns [`crate::Error::UnsupportedVersion`] if the prompt declares a
/// `promptforge:` major this build does not support,
/// [`crate::Error::Parse`] if the file has no `promptforge:` version (it is not
/// a promptforge prompt), [`crate::Error::Lua`] if a Lua preamble fails,
/// [`crate::Error::Substitution`] if a `{{ }}` path cannot be resolved,
/// [`crate::Error::MissingEnv`] if the gateway client cannot be built when a
/// model call is needed, [`crate::Error::UnknownScopedTool`] if a section
/// scopes a tool name absent from `tools`, [`crate::Error::UnknownTool`] if the
/// model calls an alias absent from its effective scope,
/// [`crate::Error::NearDuplicateTools`] if two effective tools meet the picker's
/// duplicate threshold, [`crate::Error::ToolLoopExhausted`] if a section's
/// tool-call loop does not converge within its iteration cap, or any
/// transport/backend error from a model call.
pub async fn run<'a>(
    prompt: impl Into<RunPrompt<'a>>,
    args: &str,
    tools: &[&dyn Tool],
    store: &StoreRef,
    opts: RunOptions<'_>,
) -> Result<String> {
    let run_prompt = prompt.into();
    let prompt = run_prompt.prompt;
    // Gate on the declared engine major before doing any work: promptforge runs
    // only its own prompts, and refuses an unsupported major rather than
    // silently degrading. A file with no `promptforge:` version is not a
    // promptforge prompt at all, which is the caller's concern, not ours.
    match prompt.frontmatter.promptforge {
        Some(SUPPORTED_MAJOR) => {}
        Some(other) => return Err(Error::UnsupportedVersion(other)),
        None => {
            return Err(Error::Parse(
                "not a promptforge prompt: no promptforge version".into(),
            ));
        }
    }

    let RunOptions {
        execution,
        observer,
        client,
        debug,
    } = opts;
    let registry = ToolRegistry::new(tools.iter().copied());
    let prompt_section = prompt.title.as_str();
    observer.observe(execution, prompt_section, detail::RUN_STARTED);

    // The turn count is threaded through the whole run so `RunFinished` can
    // report the total even when a section fails part way through it.
    let mut turns: u32 = 0;
    let result = run_sections(
        prompt,
        run_prompt.bound,
        args,
        &registry,
        store,
        execution,
        observer,
        client,
        debug,
        &mut turns,
    )
    .await;

    observer.observe(
        execution,
        prompt_section,
        if result.is_ok() {
            detail::RUN_SUCCEEDED
        } else {
            detail::RUN_FAILED
        },
    );
    result
}

/// Walk the prompt's top-level sections, reporting each boundary, and return
/// the run's result.
///
/// Split out of [`run`] so that every way the walk can end - a Lua return, an
/// error, running off the last section - passes through one place that emits
/// the run's final observation.
///
/// # Errors
/// Returns the same errors as [`run`], which documents them.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the lifecycle keeps its borrowed run inputs explicit and linear so every early failure can tear down its owned section VM before returning"
)]
async fn run_sections(
    prompt: &Prompt,
    bound: Option<&BoundPrompt>,
    args: &str,
    registry: &ToolRegistry<'_>,
    store: &StoreRef,
    execution: &str,
    observer: &dyn Observer,
    mut client: Option<GatewayClient>,
    debug: Option<&dyn DebugCapture>,
    turns: &mut u32,
) -> Result<String> {
    let when = now_rfc3339();
    let mut last_reply: Option<String> = None;

    // A parsed prompt without a binding pass runs with empty frozen bindings,
    // on the same validated VM path as a bound prompt: there is exactly one
    // `tools.add`, and it rejects every undeclared alias.
    let empty_bindings = ToolBindings::default();
    let empty_models = ModelBindings::default();
    let bindings = bound.map_or(&empty_bindings, BoundPrompt::bindings);
    let models = bound.map_or(&empty_models, BoundPrompt::models);

    // Resolve the tool-loop cap once: the prompt's declared budget, or the
    // runtime default when it declares none.
    let max_tool_iterations = prompt
        .frontmatter
        .max_tool_iterations
        .unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS);

    for (index, section) in prompt.sections.iter().enumerate() {
        let sys = json!({ "when": when, "now": now_rfc3339(), "id": index + 1 });

        // `completed` counts sections entered, so the first is 1. It only ever
        // grows, which is what the progress contract requires.
        observer.observe(execution, &section.name, detail::SECTION_STARTED);

        let mut vm = match prompt.shared.as_ref() {
            Some(shared) => SectionVm::new_with_shared_bindings(
                shared,
                bindings,
                models,
                execution,
                observer,
                &section.name,
            )?,
            None => SectionVm::new(None, execution, observer, &section.name)?,
        };
        if let Err(error) = vm.inject_host(args, &sys, store, last_reply.as_deref()) {
            vm.teardown(observer, &section.name);
            return Err(error);
        }

        let has_children = !section.children.is_empty();
        let preamble_return = if let Some(program) = &section.preamble {
            if has_children {
                let children = &section.children;
                let fanout_store = store.clone();
                let fanout_args = args.to_string();
                let fanout_execution = execution.to_string();
                let fanout_when = when.clone();
                let fanout_last_reply = last_reply.clone();
                let fanout_shared = prompt.shared.clone();
                let fanout_bindings = bindings.clone();
                let fanout_models = models.clone();
                let fanout_client = client.clone();
                let fanout_max_iters = max_tool_iterations;
                match vm.run_preamble_with_fanout(
                    program,
                    observer,
                    &section.name,
                    |worker_heading, list_heading| {
                        make_fanout_callback(
                            &worker_heading,
                            &list_heading,
                            children,
                            &fanout_args,
                            &fanout_store,
                            &fanout_execution,
                            observer,
                            fanout_client.as_ref(),
                            debug,
                            fanout_shared.as_ref(),
                            &fanout_bindings,
                            &fanout_models,
                            bound,
                            registry,
                            fanout_max_iters,
                            fanout_last_reply.as_deref(),
                            &fanout_when,
                            index + 1,
                        )
                    },
                ) {
                    Ok(returned) => returned,
                    Err(error) => {
                        vm.teardown(observer, &section.name);
                        return Err(error);
                    }
                }
            } else {
                match vm.run_preamble(program, observer, &section.name) {
                    Ok(returned) => returned,
                    Err(error) => {
                        vm.teardown(observer, &section.name);
                        return Err(error);
                    }
                }
            }
        } else {
            None
        };
        if let Some(value) = preamble_return {
            vm.teardown(observer, &section.name);
            observer.observe(execution, &section.name, detail::SECTION_FINISHED);
            return Ok(value);
        }

        // Every VM closes declaration recording before reply binding or epilog
        // execution; a prompt with empty bindings closes to an empty scope.
        let scopes = match vm.close_scopes(observer, &section.name) {
            Ok(scopes) => scopes,
            Err(error) => {
                vm.teardown(observer, &section.name);
                return Err(error);
            }
        };
        let scope = scopes.tools;

        let sys = if let Some(model_binding) = scopes.model.as_ref() {
            let enriched = crate::lua::enrich_sys_model(&sys, model_binding);
            if let Err(error) = vm.re_seal_sys(&enriched) {
                vm.teardown(observer, &section.name);
                return Err(error);
            }
            enriched
        } else {
            sys
        };

        let var = match vm.var() {
            Ok(var) => var,
            Err(error) => {
                vm.teardown(observer, &section.name);
                return Err(error);
            }
        };
        let prose = match subst::substitute(
            &section.prose,
            args,
            last_reply.as_deref(),
            None,
            &var,
            &sys,
        ) {
            Ok(prose) => prose,
            Err(error) => {
                vm.teardown(observer, &section.name);
                return Err(error);
            }
        };
        if !prose.trim().is_empty() {
            let Some(model_binding) = scopes.model else {
                vm.teardown(observer, &section.name);
                return Err(Error::ModelRequired {
                    section: section.name.clone(),
                });
            };
            let completion_options = model_binding.completion_options();
            let (schemas, dispatch) = match bound {
                Some(bound) => {
                    match prepare_effective_scope(
                        bound,
                        &scope,
                        registry,
                        execution,
                        observer,
                        &section.name,
                    ) {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            vm.teardown(observer, &section.name);
                            return Err(error);
                        }
                    }
                }
                None => (Vec::new(), BTreeMap::new()),
            };
            if client.is_none() {
                match GatewayClient::from_env() {
                    Ok(new_client) => client = Some(new_client),
                    Err(error) => {
                        vm.teardown(observer, &section.name);
                        return Err(error);
                    }
                }
            }
            if let Some(client) = &client {
                let text = match run_tool_loop(
                    client,
                    &schemas,
                    &dispatch,
                    registry,
                    prose,
                    max_tool_iterations,
                    SectionProgress {
                        execution,
                        observer,
                        section: &section.name,
                        turns,
                        debug,
                        completion_options: &completion_options,
                    },
                )
                .await
                {
                    Ok(text) => text,
                    Err(error) => {
                        vm.teardown(observer, &section.name);
                        return Err(error);
                    }
                };
                if let Err(error) = vm.bind_reply(&text, observer, &section.name) {
                    vm.teardown(observer, &section.name);
                    return Err(error);
                }
                last_reply = Some(text);
            }
        }

        let epilog_return = if let Some(program) = &section.epilog {
            if has_children {
                let children = &section.children;
                let fanout_store = store.clone();
                let fanout_args = args.to_string();
                let fanout_execution = execution.to_string();
                let fanout_when = when.clone();
                let fanout_last_reply = last_reply.clone();
                let fanout_shared = prompt.shared.clone();
                let fanout_bindings = bindings.clone();
                let fanout_models = models.clone();
                let fanout_client = client.clone();
                let fanout_max_iters = max_tool_iterations;
                match vm.run_epilog_with_fanout(
                    program,
                    observer,
                    &section.name,
                    |worker_heading, list_heading| {
                        make_fanout_callback(
                            &worker_heading,
                            &list_heading,
                            children,
                            &fanout_args,
                            &fanout_store,
                            &fanout_execution,
                            observer,
                            fanout_client.as_ref(),
                            debug,
                            fanout_shared.as_ref(),
                            &fanout_bindings,
                            &fanout_models,
                            bound,
                            registry,
                            fanout_max_iters,
                            fanout_last_reply.as_deref(),
                            &fanout_when,
                            index + 1,
                        )
                    },
                ) {
                    Ok(returned) => returned,
                    Err(error) => {
                        vm.teardown(observer, &section.name);
                        return Err(error);
                    }
                }
            } else {
                match vm.run_epilog(program, observer, &section.name) {
                    Ok(returned) => returned,
                    Err(error) => {
                        vm.teardown(observer, &section.name);
                        return Err(error);
                    }
                }
            }
        } else {
            None
        };
        vm.teardown(observer, &section.name);
        observer.observe(execution, &section.name, detail::SECTION_FINISHED);
        if let Some(value) = epilog_return {
            return Ok(value);
        }
    }

    // Ran off the end.
    Ok(prompt
        .frontmatter
        .default_return
        .clone()
        .or(last_reply)
        .unwrap_or_else(|| "done".to_string()))
}

#[expect(
    clippy::too_many_arguments,
    reason = "fanout callback threads all borrowed run context through to the arm executor"
)]
fn make_fanout_callback(
    worker_heading: &str,
    list_heading: &str,
    children: &[crate::parser::Section],
    args: &str,
    store: &StoreRef,
    execution: &str,
    observer: &dyn Observer,
    client: Option<&GatewayClient>,
    debug: Option<&dyn DebugCapture>,
    shared: Option<&crate::lua::LuaProgram>,
    bindings: &ToolBindings,
    models: &ModelBindings,
    bound: Option<&BoundPrompt>,
    registry: &ToolRegistry<'_>,
    max_tool_iterations: usize,
    last_reply: Option<&str>,
    when: &str,
    parent_id: usize,
) -> std::result::Result<Vec<String>, String> {
    let worker = fanout::resolve_sibling(worker_heading, children).map_err(|e| e.to_string())?;
    let list = fanout::resolve_sibling(list_heading, children).map_err(|e| e.to_string())?;
    if list.items.is_empty() {
        return Err(format!("section `{}` has no pre-parsed items", list.name));
    }
    if worker.preamble.is_none() && worker.epilog.is_none() && !worker.items.is_empty() {
        return Err(format!(
            "section `{}` is a list section, not a worker template",
            worker.name
        ));
    }

    let fanout_client = client.cloned();
    let ctx = fanout::FanoutContext {
        args,
        store,
        execution,
        observer,
        client: &fanout_client,
        debug,
        shared,
        bindings,
        models,
        bound,
        registry,
        max_tool_iterations,
        last_reply,
        when,
        parent_id,
    };

    let handle = tokio::runtime::Handle::current();
    tokio::task::block_in_place(|| {
        handle.block_on(fanout::run_fanout_arms(worker, &list.items, &ctx))
    })
    .map_err(|e| e.to_string())
}

pub(crate) fn prepare_effective_scope(
    bound: &BoundPrompt,
    scope: &ToolScope,
    registry: &ToolRegistry<'_>,
    execution: &str,
    observer: &dyn Observer,
    section: &str,
) -> Result<(Vec<ToolSchema>, BTreeMap<String, ToolId>)> {
    observer.observe(execution, section, detail::TOOL_SCOPE_VALIDATION_STARTED);
    let result = validate_effective_scope_inner(bound, scope)
        .and_then(|()| prepare_scoped_tools(scope, registry));
    observer.observe(
        execution,
        section,
        if result.is_ok() {
            detail::TOOL_SCOPE_VALIDATION_SUCCEEDED
        } else {
            detail::TOOL_SCOPE_VALIDATION_FAILED
        },
    );
    result
}

fn validate_effective_scope_inner(bound: &BoundPrompt, scope: &ToolScope) -> Result<()> {
    let effective = scope
        .bindings()
        .iter()
        .map(crate::lua::ToolBinding::id)
        .collect::<BTreeSet<_>>();
    for pair in bound.near_duplicates() {
        let first_id = ToolId::new(pair.first.id.server(), pair.first.id.name());
        let second_id = ToolId::new(pair.second.id.server(), pair.second.id.name());
        if !effective.contains(&first_id) || !effective.contains(&second_id) {
            continue;
        }
        let first_alias = bound.id_to_alias().get(&first_id).cloned().ok_or_else(|| {
            Error::ToolScopeAnalysis {
                detail: "selected identity has no frozen alias".to_owned(),
            }
        })?;
        let second_alias = bound
            .id_to_alias()
            .get(&second_id)
            .cloned()
            .ok_or_else(|| Error::ToolScopeAnalysis {
                detail: "selected identity has no frozen alias".to_owned(),
            })?;
        return Err(Error::NearDuplicateTools {
            diagnostic: Box::new(NearDuplicateDiagnostic {
                first_alias,
                first_id,
                first_description: pair.first.description.clone(),
                first_annotations: pair.first.annotations,
                second_alias,
                second_id,
                second_description: pair.second.description.clone(),
                second_annotations: pair.second.annotations,
                similarity: pair.similarity,
            }),
        });
    }
    Ok(())
}

fn prepare_scoped_tools(
    scope: &ToolScope,
    registry: &ToolRegistry<'_>,
) -> Result<(Vec<ToolSchema>, BTreeMap<String, ToolId>)> {
    let mut schemas = Vec::with_capacity(scope.bindings().len());
    let mut dispatch = BTreeMap::new();
    for binding in scope.bindings() {
        let tool = registry
            .get(binding.id())
            .ok_or_else(|| Error::UnknownScopedTool(binding.alias().to_owned()))?;
        schemas.push(ToolSchema {
            name: binding.alias().to_owned(),
            description: tool.description().to_owned(),
            parameters: tool.parameters_schema(),
        });
        dispatch.insert(binding.alias().to_owned(), binding.id().clone());
    }
    Ok((schemas, dispatch))
}

/// What one section's tool loop needs to report itself: where observations go, which
/// section they belong to, and the run-wide turn counter it advances.
///
/// Bundled rather than passed as three parameters so the loop's signature stays
/// readable, and so the counter is a run-wide total rather than a per-section
/// one.
pub(crate) struct SectionProgress<'a> {
    /// The identifier every observation from this loop carries.
    pub(crate) execution: &'a str,
    /// Where the loop reports its turns and tool calls.
    pub(crate) observer: &'a dyn Observer,
    /// The heading text every observation from this loop carries.
    pub(crate) section: &'a str,
    /// The run's model-turn total, advanced once per round trip.
    pub(crate) turns: &'a mut u32,
    /// Opt-in raw request/response capture for each model turn.
    pub(crate) debug: Option<&'a dyn DebugCapture>,
    /// Per-call model fields from the section's selected binding.
    pub(crate) completion_options: &'a CompletionOptions,
}

/// Drive one section's model call to a final text reply, dispatching any tool
/// calls the model requests along the way.
///
/// The conversation starts with the section's prose as a `user` turn. Each
/// round trip either yields text (returned immediately) or a batch of tool
/// calls; for the latter, the assistant turn is echoed back verbatim, each tool
/// is dispatched and its result appended as a `tool` turn, and the conversation
/// is re-sent. The loop is capped at `max_tool_iterations` round trips.
///
/// # Errors
/// Returns [`Error::UnknownTool`] if the model calls an alias absent from
/// `dispatch`,
/// [`Error::ToolLoopExhausted`] if the cap is hit without a text reply, or any
/// transport/backend error from a model call or a tool's own failure.
pub(crate) async fn run_tool_loop(
    client: &GatewayClient,
    schemas: &[ToolSchema],
    dispatch: &BTreeMap<String, ToolId>,
    registry: &ToolRegistry<'_>,
    prose: String,
    max_tool_iterations: usize,
    progress: SectionProgress<'_>,
) -> Result<String> {
    let SectionProgress {
        execution,
        observer,
        section,
        turns,
        debug,
        completion_options,
    } = progress;
    let mut conversation = vec![Message::user(prose)];
    let tool_arg = if schemas.is_empty() {
        None
    } else {
        Some(schemas)
    };

    // One nonce per section (per loop invocation) tags every untrusted result's
    // guard block, so the close tag is unguessable by any fetched content.
    let nonce = untrusted::nonce();

    for _ in 0..max_tool_iterations {
        let completion = client
            .complete(&conversation, tool_arg, completion_options)
            .await;
        if completion.is_err() {
            observer.observe(execution, section, detail::MODEL_TURN_FAILED);
        }
        let completion = completion?;

        // A round trip that produced a reply is a turn, whether the reply is
        // the section's final text or a batch of tool calls.
        *turns = turns.saturating_add(1);
        if let Some(capture) = debug {
            capture.on_event(
                execution,
                section,
                *turns,
                DebugEvent::Request {
                    body: completion.request_body,
                },
            );
            capture.on_event(
                execution,
                section,
                *turns,
                DebugEvent::Response {
                    body: completion.response_body.clone(),
                    finish_reason: completion.finish_reason.clone(),
                    reasoning_content: completion.reasoning_content.clone(),
                },
            );
        }
        observer.observe(execution, section, detail::MODEL_TURN_COMPLETED);

        match completion.result {
            CompletionResult::Text(text) => {
                // Empty final text never reaches here: the normalizer hard-fails
                // with EmptyModelReply (observed as MODEL_TURN_FAILED above).
                // Truncation is reported only when non-empty text arrived.
                if completion.finish_reason.as_deref() == Some("length") {
                    observer.observe(execution, section, detail::MODEL_TURN_TRUNCATED);
                }
                return Ok(text);
            }
            CompletionResult::ToolCalls(calls) => {
                // Echo the assistant's tool-call turn back into the history. The
                // parsed `ToolCall`s are reconstructed into the raw OpenAI wire
                // shape (`function.arguments` re-encoded as a JSON string).
                let raw_calls = calls
                    .iter()
                    .map(|call| {
                        json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments.to_string(),
                            },
                        })
                    })
                    .collect();
                conversation.push(Message::assistant_tool_calls(raw_calls));

                // Dispatch each requested tool and append its result.
                for call in &calls {
                    let Some(id) = dispatch.get(&call.name) else {
                        observer.observe(execution, section, detail::TOOL_CALL_FAILED);
                        return Err(Error::UnknownTool(call.name.clone()));
                    };
                    let Some(tool) = registry.get(id) else {
                        observer.observe(execution, section, detail::TOOL_CALL_FAILED);
                        return Err(Error::UnknownScopedTool(call.name.clone()));
                    };
                    let result = tool.call(call.arguments.clone()).await;
                    observer.observe(
                        execution,
                        section,
                        if result.is_ok() {
                            detail::TOOL_CALL_SUCCEEDED
                        } else {
                            detail::TOOL_CALL_FAILED
                        },
                    );
                    let result = result?;
                    // An untrusted tool's result is wrapped in a guard block
                    // before it enters the history; a trusted tool's is pushed
                    // verbatim.
                    let result = if tool.untrusted_output() {
                        untrusted::wrap(&result, &nonce)
                    } else {
                        result
                    };
                    conversation.push(Message::tool(call.id.clone(), result));
                }
            }
        }
    }

    Err(Error::ToolLoopExhausted)
}

/// The current UTC time as an RFC 3339 string, or empty on a formatting error.
pub(crate) fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
