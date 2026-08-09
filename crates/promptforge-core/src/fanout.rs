//! Explicit fanout: map a worker section over a list of items.
//!
//! A parent section's Lua epilog calls `fanout("### Worker", "### List")` to
//! run the worker template once per item parsed from the list section. Arms
//! execute concurrently on a [`tokio::task::JoinSet`]; each gets a fresh
//! [`SectionVm`] with `item` and `sys.taskid` injected. The invoker receives an
//! ordered Lua table of structured arm results (`.text`, `.ok`, `.item`,
//! `.exhausted`). Fatal arm errors abort siblings;
//! [`Error::ToolLoopExhausted`] soft-degrades to an incomplete stub.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::bind::BoundPrompt;
use crate::cancel;
use crate::client::GatewayClient;
use crate::debug::{DebugCapture, DebugEvent};
use crate::lua::{LuaFanoutResult, LuaProgram, SectionVm, ToolBindings};
use crate::model::ModelBindings;
use crate::observe::{Observer, detail};
use crate::parser::Section;
use crate::store::StoreRef;
use crate::tools::SharedTools;
use crate::{Error, Result, subst};

/// Resolves a heading string like `"### Name"` against a list of sibling
/// sections, returning the matching section.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading does not start with `#` markers,
/// or when no sibling matches. The error message lists available siblings.
pub(crate) fn resolve_sibling<'a>(heading: &str, siblings: &'a [Section]) -> Result<&'a Section> {
    let stripped = heading.trim();
    if !stripped.starts_with('#') {
        return Err(Error::Lua(format!(
            "fanout heading must include ### markers, got bare name: {stripped}"
        )));
    }
    let level_end = stripped.find(|c: char| c != '#').unwrap_or(stripped.len());
    let name = stripped[level_end..].trim();
    if name.is_empty() {
        return Err(Error::Lua(format!(
            "fanout heading has no name: {stripped}"
        )));
    }

    for section in siblings {
        if section.name == name {
            return Ok(section);
        }
    }

    let available: Vec<String> = siblings
        .iter()
        .map(|s| format!("{} {}", "#".repeat(s.level.into()), s.name))
        .collect();
    Err(Error::Lua(format!(
        "fanout heading `{stripped}` not found; available siblings: {}",
        available.join(", ")
    )))
}

/// Everything a fanout needs from the invoker's context.
pub(crate) struct FanoutContext<'a> {
    pub args: &'a str,
    pub store: &'a StoreRef,
    pub execution: &'a str,
    pub observer: &'a dyn Observer,
    pub client: &'a Option<GatewayClient>,
    pub debug: Option<&'a dyn DebugCapture>,
    pub shared: Option<&'a LuaProgram>,
    pub bindings: &'a ToolBindings,
    pub models: &'a ModelBindings,
    pub bound: Option<&'a BoundPrompt>,
    pub shared_tools: &'a SharedTools,
    pub max_tool_iterations: usize,
    pub last_reply: Option<&'a str>,
    pub when: &'a str,
    /// The 1-based id of the parent section that initiated the fanout.
    pub parent_id: usize,
}

/// Runs the worker section template once per item, concurrently.
///
/// Returns the ordered structured results from each arm (list order, not
/// finish order).
///
/// # Errors
/// Fatal arm errors abort siblings; tool-loop exhaustion soft-degrades.
pub(crate) async fn run_fanout_arms(
    worker: &Section,
    items: &[String],
    ctx: &FanoutContext<'_>,
) -> Result<Vec<LuaFanoutResult>> {
    let turns = Arc::new(AtomicU32::new(0));
    let (observe_tx, mut observe_rx) = mpsc::unbounded_channel::<(String, String)>();
    let (debug_tx, mut debug_rx) = mpsc::unbounded_channel::<DebugMsg>();
    let proxy_observer = Arc::new(ProxyObserver { tx: observe_tx });
    let proxy_debug = ctx.debug.map(|_| {
        Arc::new(ProxyDebugCapture {
            tx: debug_tx.clone(),
        }) as Arc<dyn DebugCapture>
    });

    let mut join_set = JoinSet::new();
    let mut replies: Vec<Option<LuaFanoutResult>> = vec![None; items.len()];

    for (index, item_text) in items.iter().enumerate() {
        let payload = ArmPayload {
            worker: worker.clone(),
            item_text: item_text.clone(),
            index,
            store: ctx.store.clone(),
            client: ctx.client.clone(),
            args: ctx.args.to_owned(),
            execution: ctx.execution.to_owned(),
            when: ctx.when.to_owned(),
            last_reply: ctx.last_reply.map(str::to_owned),
            shared: ctx.shared.cloned(),
            bindings: ctx.bindings.clone(),
            models: ctx.models.clone(),
            bound: ctx.bound.cloned(),
            shared_tools: ctx.shared_tools.clone(),
            max_tool_iterations: ctx.max_tool_iterations,
            parent_id: ctx.parent_id,
            turns: Arc::clone(&turns),
            observer: Arc::clone(&proxy_observer),
            debug: proxy_debug.clone(),
        };
        join_set.spawn(async move { run_one_arm(payload).await });
    }

    // Drop the unused sender clone so the debug channel can close when arms finish.
    drop(debug_tx);

    loop {
        tokio::select! {
            biased;
            () = cancel::wait_cancelled() => {
                abort_fanout_arms(&mut join_set, ctx, &mut observe_rx, &mut debug_rx).await;
                return Err(Error::Interrupted);
            }
            Some((section, detail)) = observe_rx.recv() => {
                ctx.observer.observe(ctx.execution, &section, &detail);
            }
            Some(msg) = debug_rx.recv() => {
                if let Some(capture) = ctx.debug {
                    capture.on_event(ctx.execution, &msg.section, msg.turn_index, msg.event);
                }
            }
            joined = join_set.join_next() => {
                match joined {
                    None => break,
                    Some(Ok(Ok((index, reply)))) => {
                        replies[index] = Some(reply);
                    }
                    Some(Ok(Err(error))) => {
                        abort_fanout_arms(&mut join_set, ctx, &mut observe_rx, &mut debug_rx).await;
                        return Err(error);
                    }
                    Some(Err(join_error)) if join_error.is_cancelled() => {}
                    Some(Err(join_error)) => {
                        abort_fanout_arms(&mut join_set, ctx, &mut observe_rx, &mut debug_rx).await;
                        return Err(Error::Lua(format!(
                            "fanout arm join failed: {join_error}"
                        )));
                    }
                }
            }
        }
    }

    drain_side_channels(ctx, &mut observe_rx, &mut debug_rx);

    let mut ordered = Vec::with_capacity(replies.len());
    for (index, reply) in replies.into_iter().enumerate() {
        match reply {
            Some(result) => ordered.push(result),
            None => {
                return Err(Error::Lua(format!(
                    "fanout arm {} finished without a reply",
                    index + 1
                )));
            }
        }
    }
    Ok(ordered)
}

async fn abort_fanout_arms(
    join_set: &mut JoinSet<Result<(usize, LuaFanoutResult)>>,
    ctx: &FanoutContext<'_>,
    observe_rx: &mut mpsc::UnboundedReceiver<(String, String)>,
    debug_rx: &mut mpsc::UnboundedReceiver<DebugMsg>,
) {
    join_set.abort_all();
    while join_set.join_next().await.is_some() {}
    drain_side_channels(ctx, observe_rx, debug_rx);
}

fn drain_side_channels(
    ctx: &FanoutContext<'_>,
    observe_rx: &mut mpsc::UnboundedReceiver<(String, String)>,
    debug_rx: &mut mpsc::UnboundedReceiver<DebugMsg>,
) {
    while let Ok((section, detail)) = observe_rx.try_recv() {
        ctx.observer.observe(ctx.execution, &section, &detail);
    }
    while let Ok(msg) = debug_rx.try_recv() {
        if let Some(capture) = ctx.debug {
            capture.on_event(ctx.execution, &msg.section, msg.turn_index, msg.event);
        }
    }
}

struct ArmPayload {
    worker: Section,
    item_text: String,
    index: usize,
    store: StoreRef,
    client: Option<GatewayClient>,
    args: String,
    execution: String,
    when: String,
    last_reply: Option<String>,
    shared: Option<LuaProgram>,
    bindings: ToolBindings,
    models: ModelBindings,
    bound: Option<BoundPrompt>,
    shared_tools: SharedTools,
    max_tool_iterations: usize,
    parent_id: usize,
    turns: Arc<AtomicU32>,
    observer: Arc<ProxyObserver>,
    debug: Option<Arc<dyn DebugCapture>>,
}

struct ProxyObserver {
    tx: mpsc::UnboundedSender<(String, String)>,
}

impl Observer for ProxyObserver {
    fn observe(&self, _execution: &str, section: &str, detail: &str) {
        // Parent may already have returned after fail-fast drain/drop.
        let _ = self.tx.send((section.to_owned(), detail.to_owned()));
    }
}

struct DebugMsg {
    section: String,
    turn_index: u32,
    event: DebugEvent,
}

struct ProxyDebugCapture {
    tx: mpsc::UnboundedSender<DebugMsg>,
}

impl DebugCapture for ProxyDebugCapture {
    fn on_event(&self, _execution: &str, section: &str, turn_index: u32, event: DebugEvent) {
        // Parent may already have returned after fail-fast drain/drop.
        let _ = self.tx.send(DebugMsg {
            section: section.to_owned(),
            turn_index,
            event,
        });
    }
}

/// Runs one fanout arm to completion.
#[expect(
    clippy::too_many_lines,
    reason = "the arm lifecycle is a linear sequence of fallible steps with per-step cleanup"
)]
async fn run_one_arm(payload: ArmPayload) -> Result<(usize, LuaFanoutResult)> {
    let ArmPayload {
        worker,
        item_text,
        index,
        store,
        client,
        args,
        execution,
        when,
        last_reply,
        shared,
        bindings,
        models,
        bound,
        shared_tools,
        max_tool_iterations,
        parent_id,
        turns,
        observer,
        debug,
    } = payload;

    let taskid = (index + 1).to_string();
    let observer = observer.as_ref() as &dyn Observer;
    observer.observe(&execution, &worker.name, detail::FANOUT_ARM_STARTED);

    let mut vm = match shared.as_ref() {
        Some(program) => SectionVm::new_with_shared_bindings(
            program,
            &bindings,
            &models,
            &execution,
            observer,
            &worker.name,
        )?,
        None => SectionVm::new(None, &execution, observer, &worker.name)?,
    };

    let sys = json!({
        "when": when,
        "now": crate::execute::now_rfc3339(),
        "id": parent_id,
        "taskid": taskid,
    });

    if let Err(error) = vm.inject_host(&args, &sys, &store, last_reply.as_deref()) {
        vm.teardown(observer, &worker.name);
        return Err(error);
    }

    if let Err(error) = vm.set_global_string("item", &item_text) {
        vm.teardown(observer, &worker.name);
        return Err(error);
    }

    let prologue_return = if let Some(program) = worker.prologue() {
        match vm.run_prologue(program, observer, &worker.name) {
            Ok(returned) => returned,
            Err(error) => {
                vm.teardown(observer, &worker.name);
                return Err(error);
            }
        }
    } else {
        None
    };

    if let Some(value) = prologue_return {
        vm.teardown(observer, &worker.name);
        observer.observe(&execution, &worker.name, detail::FANOUT_ARM_FINISHED);
        return Ok((index, LuaFanoutResult::success(&item_text, value)));
    }

    let scopes = match vm.close_scopes(observer, &worker.name) {
        Ok(scopes) => scopes,
        Err(error) => {
            vm.teardown(observer, &worker.name);
            return Err(error);
        }
    };
    let scope = scopes.tools;
    let counts = match vm.install_tool_call_counts(&scope) {
        Ok(c) => Some(c),
        Err(error) => {
            vm.teardown(observer, &worker.name);
            return Err(error);
        }
    };

    let sys = if let Some(model_binding) = scopes.model.as_ref() {
        let enriched = crate::lua::enrich_sys_model(&sys, model_binding);
        if let Err(error) = vm.re_seal_sys(&enriched) {
            vm.teardown(observer, &worker.name);
            return Err(error);
        }
        enriched
    } else {
        sys
    };

    let var = match vm.var() {
        Ok(var) => var,
        Err(error) => {
            vm.teardown(observer, &worker.name);
            return Err(error);
        }
    };
    let prose = match subst::substitute(
        worker.prose(),
        &args,
        last_reply.as_deref(),
        Some(&item_text),
        &var,
        &sys,
    ) {
        Ok(prose) => prose,
        Err(error) => {
            vm.teardown(observer, &worker.name);
            return Err(error);
        }
    };

    let mut arm_reply: Option<String> = None;
    if !prose.trim().is_empty() {
        let Some(model_binding) = scopes.model else {
            vm.teardown(observer, &worker.name);
            return Err(Error::ModelRequired {
                section: worker.name.clone(),
            });
        };
        let completion_options = model_binding.completion_options();
        let registry = shared_tools.registry();
        let (schemas, dispatch) = match bound.as_ref() {
            Some(bound_prompt) => {
                match crate::execute::prepare_effective_scope(
                    bound_prompt,
                    &scope,
                    &registry,
                    &execution,
                    observer,
                    &worker.name,
                ) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        vm.teardown(observer, &worker.name);
                        return Err(error);
                    }
                }
            }
            None => (Vec::new(), BTreeMap::new()),
        };
        if let Some(client) = client.as_ref() {
            let global_aliases = bound.as_ref().map(BoundPrompt::alias_to_id);
            let debug_ref = debug.as_deref();
            let text = match crate::execute::run_tool_loop(
                client,
                &schemas,
                &dispatch,
                &registry,
                prose,
                max_tool_iterations,
                crate::execute::SectionProgress {
                    execution: &execution,
                    observer,
                    section: &worker.name,
                    turns: turns.as_ref(),
                    debug: debug_ref,
                    completion_options: &completion_options,
                },
                counts.as_ref(),
                global_aliases,
            )
            .await
            {
                Ok(text) => text,
                // One stuck arm must not kill sibling evidence facets.
                Err(Error::ToolLoopExhausted) => {
                    vm.teardown(observer, &worker.name);
                    let stub = format!(
                        "## {item_text}\n\nUNKNOWN\n\n(section incomplete: tool loop exhausted)"
                    );
                    observer.observe(&execution, &worker.name, detail::FANOUT_ARM_FINISHED);
                    return Ok((index, LuaFanoutResult::exhausted_stub(&item_text, stub)));
                }
                Err(error) => {
                    vm.teardown(observer, &worker.name);
                    return Err(error);
                }
            };
            if let Err(error) = vm.bind_reply(&text, observer, &worker.name) {
                vm.teardown(observer, &worker.name);
                return Err(error);
            }
            arm_reply = Some(text);
        }
    }

    let epilog_return = if let Some(program) = worker.epilog() {
        match vm.run_epilog(program, observer, &worker.name) {
            Ok(returned) => returned,
            Err(error) => {
                vm.teardown(observer, &worker.name);
                return Err(error);
            }
        }
    } else {
        None
    };

    vm.teardown(observer, &worker.name);
    observer.observe(&execution, &worker.name, detail::FANOUT_ARM_FINISHED);

    let text = epilog_return.or(arm_reply).unwrap_or_default();
    Ok((index, LuaFanoutResult::success(item_text, text)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Block;

    #[test]
    fn resolve_sibling_finds_exact_match() {
        let sections = vec![
            Section {
                name: "Worker".to_string(),
                level: 3,
                blocks: vec![Block::Prose {
                    text: String::new(),
                    loop_capable: true,
                }],
                children: Vec::new(),
                items: Vec::new(),
            },
            Section {
                name: "Topics".to_string(),
                level: 3,
                blocks: vec![Block::Prose {
                    text: String::new(),
                    loop_capable: true,
                }],
                children: Vec::new(),
                items: vec!["a".to_string()],
            },
        ];
        let found = resolve_sibling("### Worker", &sections).expect("must resolve");
        assert_eq!(found.name, "Worker");
    }

    #[test]
    fn resolve_sibling_missing_heading_lists_available() {
        let sections = vec![Section {
            name: "Worker".to_string(),
            level: 3,
            blocks: vec![Block::Prose {
                text: String::new(),
                loop_capable: true,
            }],
            children: Vec::new(),
            items: Vec::new(),
        }];
        let err =
            resolve_sibling("### Missing", &sections).expect_err("missing heading must error");
        assert!(err.to_string().contains("### Worker"), "error was: {err}");
    }

    #[test]
    fn resolve_sibling_bare_name_errors() {
        let sections = vec![Section {
            name: "Worker".to_string(),
            level: 3,
            blocks: vec![Block::Prose {
                text: String::new(),
                loop_capable: true,
            }],
            children: Vec::new(),
            items: Vec::new(),
        }];
        let err =
            resolve_sibling("Worker", &sections).expect_err("bare name without ### must error");
        assert!(err.to_string().contains("### markers"), "error was: {err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pre_cancelled_fanout_returns_interrupted() {
        use crate::Error;
        use crate::cancel::{self, CancelHandle};
        use crate::client::GatewayClient;
        use crate::lua::LuaProgram;
        use crate::model::ModelBindings;
        use crate::observe::NullObserver;
        use crate::parser::Section;
        use crate::store::StoreRef;

        let prologue = LuaProgram::compile(
            "return item",
            "test prologue",
            1,
            "fanout-cancel-test",
            &NullObserver,
            "Worker",
        )
        .expect("test Lua must compile");
        let worker = Section {
            name: "Worker".to_string(),
            level: 3,
            blocks: vec![Block::Lua(prologue)],
            children: Vec::new(),
            items: Vec::new(),
        };
        let items = vec!["alpha".to_string(), "beta".to_string()];
        let store = StoreRef::memory();
        let bindings = ToolBindings::default();
        let models = ModelBindings::default();
        let shared_tools = SharedTools::default();
        let client: Option<GatewayClient> = None;
        let observer = NullObserver;
        let ctx = FanoutContext {
            args: "",
            store: &store,
            execution: "fanout-cancel-test",
            observer: &observer,
            client: &client,
            debug: None,
            shared: None,
            bindings: &bindings,
            models: &models,
            bound: None,
            shared_tools: &shared_tools,
            max_tool_iterations: 24,
            last_reply: None,
            when: "2026-08-08",
            parent_id: 1,
        };

        let cancel = CancelHandle::new();
        cancel.cancel();
        let error = cancel::scope(cancel, run_fanout_arms(&worker, &items, &ctx))
            .await
            .expect_err("pre-cancelled fanout must fail");
        assert!(
            matches!(error, Error::Interrupted),
            "expected Interrupted, got {error}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn model_required_when_arm_prose_has_no_binding() {
        use crate::Error;
        use crate::client::GatewayClient;
        use crate::model::ModelBindings;
        use crate::observe::NullObserver;
        use crate::parser::Section;
        use crate::store::StoreRef;

        let worker = Section {
            name: "Worker".to_string(),
            level: 3,
            blocks: vec![Block::Prose {
                text: "Ask the model about {{ item }}.".to_string(),
                loop_capable: true,
            }],
            children: Vec::new(),
            items: Vec::new(),
        };
        let items = vec!["alpha".to_string()];
        let store = StoreRef::memory();
        let bindings = ToolBindings::default();
        let models = ModelBindings::default();
        let shared_tools = SharedTools::default();
        let client: Option<GatewayClient> = None;
        let observer = NullObserver;
        let ctx = FanoutContext {
            args: "",
            store: &store,
            execution: "fanout-test",
            observer: &observer,
            client: &client,
            debug: None,
            shared: None,
            bindings: &bindings,
            models: &models,
            bound: None,
            shared_tools: &shared_tools,
            max_tool_iterations: 24,
            last_reply: None,
            when: "2026-08-08",
            parent_id: 1,
        };

        let error = run_fanout_arms(&worker, &items, &ctx)
            .await
            .expect_err("non-empty arm prose without a model binding must fail");
        assert!(
            matches!(error, Error::ModelRequired { .. }),
            "expected ModelRequired, got {error}"
        );
        assert!(
            error
                .to_string()
                .contains("model binding required for section Worker"),
            "error must name the worker section: {error}"
        );
    }
}
