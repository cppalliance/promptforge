//! Explicit fanout: map a worker section over a list of items.
//!
//! A parent section's Lua epilog calls `fanout("### Worker", "### List")` to
//! run the worker template once per item parsed from the list section. Arms
//! execute concurrently on a [`tokio::task::JoinSet`]; each gets a fresh
//! [`SectionVm`] with `item` and `sys.taskid` injected. The invoker receives an
//! ordered Lua table of structured arm results (`.text`, `.ok`, `.item`,
//! `.exhausted`). Fatal arm errors abort siblings;
//! [`Error::ToolLoopExhausted`] soft-degrades to an incomplete stub.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::cancel;
use crate::client::GatewayClient;
use crate::debug::{DebugCapture, DebugEvent};
use crate::lua::{LuaFanoutResult, LuaProgram, SectionVm, ToolBindings};
use crate::model::ModelBindings;
use crate::observe::{Observation, Observer, detail};
use crate::parser::Section;
use crate::store::StoreRef;
use crate::tools::SharedTools;
use crate::{Error, Result, subst};

/// Parses a fanout heading like `"### Name"` into an exact `(level, name)`
/// address.
///
/// The marker run must be one-or-more `#`, immediately followed by whitespace,
/// then a non-empty name. `"###Name"` (no whitespace) and a bare name are both
/// rejected, so a malformed heading can never be silently reinterpreted.
///
/// # Errors
/// Returns [`Error::Lua`] when the marker run is absent, is not followed by
/// whitespace, or the name is empty.
fn parse_heading_address(heading: &str) -> Result<(usize, String)> {
    let stripped = heading.trim();
    let level = stripped.chars().take_while(|&c| c == '#').count();
    if level == 0 {
        return Err(Error::Lua(format!(
            "fanout heading must include ### markers, got bare name: {stripped}"
        )));
    }
    // The `#` run is ASCII, so a byte slice at `level` is a valid boundary.
    let rest = &stripped[level..];
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return Err(Error::Lua(format!(
            "fanout heading must have whitespace after the {} markers: {stripped}",
            "#".repeat(level)
        )));
    }
    let name = rest.trim();
    if name.is_empty() {
        return Err(Error::Lua(format!(
            "fanout heading has no name: {stripped}"
        )));
    }
    Ok((level, name.to_owned()))
}

/// Resolves a heading string like `"### Name"` against a list of sibling
/// sections, returning the single matching section.
///
/// The heading is parsed into an exact `(level, name)` address; a sibling
/// matches only when BOTH its level and name are equal. Zero matches and more
/// than one match are both rejected, so an ambiguous or level-mismatched
/// heading never resolves to an arbitrary first hit.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading is malformed (see
/// [`parse_heading_address`]), when no sibling matches the exact address, or
/// when more than one sibling matches. The error message lists available
/// siblings.
pub(crate) fn resolve_sibling<'a>(heading: &str, siblings: &'a [Section]) -> Result<&'a Section> {
    let (level, name) = parse_heading_address(heading)?;

    let mut matches = siblings
        .iter()
        .filter(|section| usize::from(section.level) == level && section.name == name);
    let Some(found) = matches.next() else {
        let available: Vec<String> = siblings
            .iter()
            .map(|s| format!("{} {}", "#".repeat(s.level.into()), s.name))
            .collect();
        return Err(Error::Lua(format!(
            "fanout heading `{}` not found; available siblings: {}",
            heading.trim(),
            available.join(", ")
        )));
    };
    if matches.next().is_some() {
        return Err(Error::Lua(format!(
            "fanout heading `{}` is ambiguous; more than one sibling matches {} {name}",
            heading.trim(),
            "#".repeat(level)
        )));
    }
    Ok(found)
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
    pub analysis: &'a crate::execute::ToolAnalysis,
    pub shared_tools: &'a SharedTools,
    pub max_tool_iterations: usize,
    /// Maximum number of arms permitted to execute concurrently.
    pub fanout_concurrency: NonZeroUsize,
    /// Maximum number of items a single fanout may map over.
    pub max_fanout_items: NonZeroUsize,
    /// Per-arm Lua heap ceiling.
    pub lua_memory_bytes: usize,
    /// Per-arm Lua `log()` event budget.
    pub lua_log_events: u32,
    pub last_reply: Option<&'a str>,
    pub when: &'a str,
    /// The 1-based id of the parent section that initiated the fanout.
    pub parent_id: usize,
    /// Total H2 section count in the top-level prompt.
    pub section_count: usize,
}

/// Runs the worker section template once per item, concurrently.
///
/// Returns the ordered structured results from each arm (list order, not
/// finish order).
///
/// # Errors
/// Fatal arm errors abort siblings; tool-loop exhaustion soft-degrades.
#[expect(
    clippy::too_many_lines,
    reason = "the scheduler is one cohesive unit: item-cap check, arm spawner, windowed dispatch, and the select! drain loop"
)]
pub(crate) async fn run_fanout_arms(
    worker: &Section,
    items: &[String],
    ctx: &FanoutContext<'_>,
) -> Result<Vec<LuaFanoutResult>> {
    // Reject an oversized list before scheduling anything, so a pathological
    // prompt cannot allocate an unbounded number of arms.
    if items.len() > ctx.max_fanout_items.get() {
        return Err(Error::Lua(format!(
            "fanout list has {} items, exceeding the maximum of {}",
            items.len(),
            ctx.max_fanout_items.get()
        )));
    }

    let turns = Arc::new(AtomicU32::new(0));
    // A spawned arm does not inherit the run's task-local CancelHandle, so
    // capture it here (on the parent task) and carry an explicit clone into every
    // arm payload; each arm re-installs it so its own Lua hook and tool loop
    // observe cancellation (PF-CANCEL-002).
    let arm_cancel = cancel::current();
    // Side channels carry report-only observation/debug traffic. They are BOUNDED
    // so a burst of arm events cannot grow memory without limit. On overload the
    // proxies drop events (see `ProxyObserver`/`ProxyDebugCapture`) rather than
    // block an arm, so back-pressure can never alter execution results - only the
    // completeness of best-effort progress reporting.
    let (observe_tx, mut observe_rx) =
        mpsc::channel::<(String, Observation)>(SIDE_CHANNEL_CAPACITY);
    let (debug_tx, mut debug_rx) = mpsc::channel::<DebugMsg>(SIDE_CHANNEL_CAPACITY);
    let proxy_observer = Arc::new(ProxyObserver { tx: observe_tx });
    let proxy_debug = ctx.debug.map(|_| {
        Arc::new(ProxyDebugCapture {
            tx: debug_tx.clone(),
        }) as Arc<dyn DebugCapture>
    });

    let mut join_set: JoinSet<Result<(usize, LuaFanoutResult)>> = JoinSet::new();
    let mut replies: Vec<Option<LuaFanoutResult>> = (0..items.len()).map(|_| None).collect();

    // Spawns arm `index`, cloning only that arm's inputs. Concurrency is bounded
    // by only ever having `ArmWindow`-approved arms resident in the `JoinSet`.
    let spawn_arm = |index: usize, join_set: &mut JoinSet<Result<(usize, LuaFanoutResult)>>| {
        let payload = ArmPayload {
            worker: worker.clone(),
            item_text: items[index].clone(),
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
            analysis: ctx.analysis.clone(),
            shared_tools: ctx.shared_tools.clone(),
            max_tool_iterations: ctx.max_tool_iterations,
            lua_memory_bytes: ctx.lua_memory_bytes,
            lua_log_events: ctx.lua_log_events,
            parent_id: ctx.parent_id,
            section_count: ctx.section_count,
            turns: Arc::clone(&turns),
            observer: Arc::clone(&proxy_observer),
            debug: proxy_debug.clone(),
            cancel: arm_cancel.clone(),
        };
        join_set.spawn(run_one_arm(payload));
    };

    // At most `fanout_concurrency` arms are resident at once: seed the initial
    // window, then schedule the next queued item whenever one completes.
    let mut window = ArmWindow::new(items.len(), ctx.fanout_concurrency);
    while let Some(index) = window.take_next() {
        spawn_arm(index, &mut join_set);
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
            Some((section, event)) = observe_rx.recv() => {
                ctx.observer.observe(ctx.execution, &section, event);
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
                        window.complete_one();
                        while let Some(next) = window.take_next() {
                            spawn_arm(next, &mut join_set);
                        }
                    }
                    Some(Ok(Err(error))) => {
                        abort_fanout_arms(&mut join_set, ctx, &mut observe_rx, &mut debug_rx).await;
                        return Err(error);
                    }
                    Some(Err(join_error)) if join_error.is_cancelled() => {}
                    Some(Err(join_error)) => {
                        abort_fanout_arms(&mut join_set, ctx, &mut observe_rx, &mut debug_rx).await;
                        // Keep the structured JoinError as the error source; it is
                        // only stringified at the Lua callback boundary.
                        return Err(Error::FanoutArmJoin(join_error));
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

/// Windowed fan-out scheduler state: never lets more than `concurrency` arms be
/// outstanding, and hands out each item index exactly once.
///
/// Kept as a small, pure type so the concurrency bound can be tested directly
/// without spinning up real arm futures.
struct ArmWindow {
    next: usize,
    count: usize,
    outstanding: usize,
    concurrency: usize,
}

impl ArmWindow {
    fn new(count: usize, concurrency: NonZeroUsize) -> Self {
        Self {
            next: 0,
            count,
            outstanding: 0,
            concurrency: concurrency.get(),
        }
    }

    /// Returns the next index to spawn when a slot is free, else `None`.
    fn take_next(&mut self) -> Option<usize> {
        if self.next < self.count && self.outstanding < self.concurrency {
            let index = self.next;
            self.next += 1;
            self.outstanding += 1;
            Some(index)
        } else {
            None
        }
    }

    /// Records that one outstanding arm has finished, freeing a slot.
    fn complete_one(&mut self) {
        self.outstanding = self.outstanding.saturating_sub(1);
    }
}

async fn abort_fanout_arms(
    join_set: &mut JoinSet<Result<(usize, LuaFanoutResult)>>,
    ctx: &FanoutContext<'_>,
    observe_rx: &mut mpsc::Receiver<(String, Observation)>,
    debug_rx: &mut mpsc::Receiver<DebugMsg>,
) {
    join_set.abort_all();
    while join_set.join_next().await.is_some() {}
    drain_side_channels(ctx, observe_rx, debug_rx);
}

fn drain_side_channels(
    ctx: &FanoutContext<'_>,
    observe_rx: &mut mpsc::Receiver<(String, Observation)>,
    debug_rx: &mut mpsc::Receiver<DebugMsg>,
) {
    while let Ok((section, event)) = observe_rx.try_recv() {
        ctx.observer.observe(ctx.execution, &section, event);
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
    analysis: crate::execute::ToolAnalysis,
    shared_tools: SharedTools,
    max_tool_iterations: usize,
    lua_memory_bytes: usize,
    lua_log_events: u32,
    parent_id: usize,
    section_count: usize,
    turns: Arc<AtomicU32>,
    observer: Arc<ProxyObserver>,
    debug: Option<Arc<dyn DebugCapture>>,
    /// Explicit cancellation handle carried across the spawn boundary, since a
    /// spawned arm does not inherit the parent task-local (PF-CANCEL-002).
    cancel: Option<cancel::CancelHandle>,
}

/// Bound on each fanout side channel (observation and debug).
///
/// Sized to absorb normal bursts while capping worst-case queued memory. On
/// overload the proxies drop events instead of blocking, so this bound never
/// changes execution results - only best-effort report completeness.
const SIDE_CHANNEL_CAPACITY: usize = 256;

struct ProxyObserver {
    tx: mpsc::Sender<(String, Observation)>,
}

impl Observer for ProxyObserver {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        // Report-only: never block an arm on a slow/full/closed consumer. A full
        // channel drops this event; the parent may also have returned already
        // after a fail-fast drain/drop. Neither can alter execution results.
        let _ = self.tx.try_send((section.to_owned(), event));
    }
}

struct DebugMsg {
    section: String,
    turn_index: u32,
    event: DebugEvent,
}

struct ProxyDebugCapture {
    tx: mpsc::Sender<DebugMsg>,
}

impl DebugCapture for ProxyDebugCapture {
    fn on_event(&self, _execution: &str, section: &str, turn_index: u32, event: DebugEvent) {
        // Report-only: a full or closed channel drops this event rather than
        // blocking the arm, so debug back-pressure cannot alter execution.
        let _ = self.tx.try_send(DebugMsg {
            section: section.to_owned(),
            turn_index,
            event,
        });
    }
}

/// Emits exactly one distinct terminal observation per fanout arm.
///
/// The arm's normal exits call [`finish`](Self::finish) with the specific
/// terminal event (succeeded / exhausted / failed). If the arm's future is
/// instead dropped before finalizing - a sibling's hard error aborts it, or the
/// run is cancelled - `Drop` emits [`detail::FANOUT_ARM_CANCELLED`]. Exactly one
/// terminal event therefore fires for every arm (FANOUT-004).
struct ArmFinalizer {
    observer: Arc<ProxyObserver>,
    execution: String,
    section: String,
    finished: bool,
}

impl ArmFinalizer {
    fn new(observer: Arc<ProxyObserver>, execution: String, section: String) -> Self {
        Self {
            observer,
            execution,
            section,
            finished: false,
        }
    }

    fn finish(&mut self, event: Observation) {
        self.finished = true;
        (self.observer.as_ref() as &dyn Observer).observe(&self.execution, &self.section, event);
    }
}

impl Drop for ArmFinalizer {
    fn drop(&mut self) {
        if !self.finished {
            (self.observer.as_ref() as &dyn Observer).observe(
                &self.execution,
                &self.section,
                detail::FANOUT_ARM_CANCELLED,
            );
        }
    }
}

/// Runs one fanout arm to completion.
///
/// VM teardown and the terminal arm observation happen in ONE epilogue
/// (FANOUT-006): the fallible body runs against a borrowed VM without any inline
/// teardown, then the epilogue tears the VM down once and records the single
/// distinct terminal event via [`ArmFinalizer`].
#[expect(
    clippy::too_many_lines,
    reason = "the arm body is one cohesive linear sequence of fallible steps"
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
        analysis,
        shared_tools,
        max_tool_iterations,
        lua_memory_bytes,
        lua_log_events,
        parent_id,
        section_count,
        turns,
        observer,
        debug,
        cancel,
    } = payload;

    let taskid = (index + 1).to_string();
    let observer_arc = observer;
    let observer = observer_arc.as_ref() as &dyn Observer;
    observer.observe(&execution, &worker.name, detail::FANOUT_ARM_STARTED);

    // The guard defaults to a CANCELLED terminal event; the epilogue below
    // upgrades it to the arm's real outcome unless the arm is aborted first.
    let mut finalizer = ArmFinalizer::new(
        Arc::clone(&observer_arc),
        execution.clone(),
        worker.name.clone(),
    );

    let mut vm = match SectionVm::new_for_section(
        shared.as_ref(),
        &bindings,
        &models,
        &execution,
        observer,
        &worker.name,
    ) {
        Ok(vm) => vm,
        Err(error) => {
            finalizer.finish(detail::FANOUT_ARM_FAILED);
            return Err(error);
        }
    };

    // The body performs no teardown; every fallible step uses `?`. It returns the
    // arm result paired with its distinct terminal event.
    let body = async {
        vm.apply_lua_limits(lua_memory_bytes, lua_log_events)?;

        let now = crate::execute::now_rfc3339_checked()?;
        let sys = json!({
            "when": when,
            "now": now,
            "id": parent_id,
            "taskid": taskid,
            "section_name": worker.name,
            "execution": execution,
            "section_count": section_count,
        });

        vm.inject_host(&args, &sys, &store, last_reply.as_deref())?;
        vm.set_global_string("item", &item_text)?;

        if let Some(program) = worker.prologue()
            && let Some(value) = vm.run_prologue(program, observer, &worker.name)?
        {
            return Ok((
                LuaFanoutResult::success(&item_text, value),
                detail::FANOUT_ARM_SUCCEEDED,
            ));
        }

        let scopes = vm.close_scopes(observer, &worker.name)?;
        let scope = scopes.tools;
        let counts = Some(vm.install_tool_call_counts(&scope)?);

        let sys = if let Some(model_binding) = scopes.model.as_ref() {
            let current = vm.current_sys(&sys)?;
            let enriched = crate::lua::enrich_sys_model(&current, model_binding);
            vm.re_seal_sys(&enriched)?;
            enriched
        } else {
            sys
        };

        let var = vm.var()?;
        let prose = subst::substitute(
            worker.prose(),
            &args,
            last_reply.as_deref(),
            Some(&item_text),
            &var,
            &sys,
        )?;

        let mut arm_reply: Option<String> = None;
        if !prose.trim().is_empty() {
            let Some(model_binding) = scopes.model else {
                return Err(Error::ModelRequired {
                    section: worker.name.clone(),
                });
            };
            let completion_options = model_binding.completion_options();
            let registry = shared_tools.registry();
            let (schemas, dispatch) = crate::execute::prepare_effective_scope(
                &analysis,
                &scope,
                &registry,
                &execution,
                observer,
                &worker.name,
            )?;
            if let Some(client) = client.as_ref() {
                let global_aliases = Some(&analysis.alias_to_id);
                let debug_ref = debug.as_deref();
                match crate::execute::run_tool_loop(
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
                    Ok((text, finish_reason)) => {
                        let current = vm.current_sys(&sys)?;
                        let enriched = crate::lua::enrich_sys_reply_finish_reason(
                            &current,
                            finish_reason.as_deref(),
                        );
                        vm.re_seal_sys(&enriched)?;
                        vm.bind_reply(&text, observer, &worker.name)?;
                        arm_reply = Some(text);
                    }
                    // One stuck arm must not kill sibling evidence facets.
                    Err(Error::ToolLoopExhausted) => {
                        let stub = format!(
                            "## {item_text}\n\nUNKNOWN\n\n(section incomplete: tool loop exhausted)"
                        );
                        return Ok((
                            LuaFanoutResult::exhausted_stub(&item_text, stub),
                            detail::FANOUT_ARM_EXHAUSTED,
                        ));
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        let epilog_return = if let Some(program) = worker.epilog() {
            vm.run_epilog(program, observer, &worker.name)?
        } else {
            None
        };

        let text = epilog_return.or(arm_reply).unwrap_or_default();
        Ok((
            LuaFanoutResult::success(item_text.clone(), text),
            detail::FANOUT_ARM_SUCCEEDED,
        ))
    };

    // Re-install the explicit cancel handle on THIS arm's task so its Lua
    // instruction hook and tool loop observe cancellation cooperatively; a
    // spawned task never inherits the parent's task-local (PF-CANCEL-002).
    let outcome: Result<(LuaFanoutResult, Observation)> = match cancel {
        Some(handle) => cancel::scope(handle, body).await,
        None => body.await,
    };

    // Single epilogue: tear the VM down once, then record exactly one terminal
    // observation matching the arm's real outcome.
    vm.teardown(observer, &worker.name);
    match outcome {
        Ok((result, event)) => {
            finalizer.finish(event);
            Ok((index, result))
        }
        Err(error) => {
            finalizer.finish(detail::FANOUT_ARM_FAILED);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

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

    fn sibling(name: &str, level: u8) -> Section {
        Section {
            name: name.to_string(),
            level,
            blocks: vec![Block::Prose {
                text: String::new(),
                loop_capable: true,
            }],
            children: Vec::new(),
            items: Vec::new(),
        }
    }

    #[test]
    fn resolve_sibling_requires_whitespace_after_markers() {
        let sections = vec![sibling("Worker", 3)];
        let err = resolve_sibling("###Worker", &sections)
            .expect_err("no whitespace after markers must error");
        assert!(err.to_string().contains("whitespace"), "error was: {err}");
    }

    #[test]
    fn resolve_sibling_requires_exact_level() {
        let sections = vec![sibling("Worker", 3)];
        // Same name, wrong marker level, must not resolve.
        let err = resolve_sibling("## Worker", &sections)
            .expect_err("a level mismatch must not resolve by name alone");
        assert!(err.to_string().contains("not found"), "error was: {err}");
        // The exact address resolves.
        let ok = resolve_sibling("### Worker", &sections).expect("exact address resolves");
        assert_eq!(ok.name, "Worker");
    }

    #[test]
    fn resolve_sibling_rejects_more_than_one_match() {
        let sections = vec![sibling("Worker", 3), sibling("Worker", 3)];
        let err = resolve_sibling("### Worker", &sections)
            .expect_err("two identical siblings must be rejected as ambiguous");
        assert!(err.to_string().contains("ambiguous"), "error was: {err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fanout_arm_join_failure_preserves_the_join_error_source() {
        // A panicked/aborted arm surfaces as `Error::FanoutArmJoin` that keeps
        // the structured `JoinError` as its `#[source]`, rather than being
        // flattened into an `Error::Lua` string that loses the cause.
        use std::error::Error as _;

        let join_error = tokio::spawn(async { panic!("arm blew up") })
            .await
            .expect_err("a panicking task must produce a JoinError");
        let error = Error::FanoutArmJoin(join_error);
        assert!(
            error.source().is_some(),
            "the JoinError must be preserved as the error source"
        );
        assert!(
            !error.to_string().contains("arm blew up"),
            "the panic payload is not stringified into the outer message"
        );
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
            NonZeroU32::new(1).expect("compile source line is non-zero"),
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
        let analysis = crate::execute::ToolAnalysis::default();
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
            analysis: &analysis,
            shared_tools: &shared_tools,
            max_tool_iterations: 24,
            fanout_concurrency: NonZeroUsize::new(8).expect("8 is non-zero"),
            max_fanout_items: NonZeroUsize::new(1024).expect("1024 is non-zero"),
            lua_memory_bytes: 64 * 1024 * 1024,
            lua_log_events: 1024,
            last_reply: None,
            when: "2026-08-08",
            parent_id: 1,
            section_count: 1,
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

    #[test]
    fn arm_window_never_exceeds_the_concurrency_limit() {
        // Drive the pure scheduler through every completion order for a few
        // sizes and prove the invariant that gates real arms: outstanding never
        // exceeds the limit, and each index is dispatched exactly once.
        for &limit in &[1usize, 2, 3, 5] {
            for &count in &[0usize, 1, 4, 9, 20] {
                let concurrency = NonZeroUsize::new(limit).expect("limit is non-zero");
                let mut window = ArmWindow::new(count, concurrency);
                let mut in_flight: Vec<usize> = Vec::new();
                let mut dispatched: Vec<usize> = Vec::new();
                let mut max_outstanding = 0usize;

                while let Some(index) = window.take_next() {
                    in_flight.push(index);
                    dispatched.push(index);
                }
                assert!(
                    in_flight.len() <= limit,
                    "initial window {} exceeded limit {limit}",
                    in_flight.len()
                );
                let mut toggle = false;
                while !in_flight.is_empty() {
                    assert!(
                        in_flight.len() <= limit,
                        "outstanding {} exceeded limit {limit}",
                        in_flight.len()
                    );
                    max_outstanding = max_outstanding.max(in_flight.len());
                    // Complete arms from alternating ends to vary the order.
                    let done = if toggle {
                        in_flight.remove(0)
                    } else {
                        in_flight.pop().expect("non-empty")
                    };
                    toggle = !toggle;
                    let _ = done;
                    window.complete_one();
                    while let Some(index) = window.take_next() {
                        in_flight.push(index);
                        dispatched.push(index);
                    }
                }

                assert!(max_outstanding <= limit);
                dispatched.sort_unstable();
                assert_eq!(
                    dispatched,
                    (0..count).collect::<Vec<_>>(),
                    "every item index must be dispatched exactly once"
                );
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fanout_rejects_a_list_over_the_item_cap() {
        use crate::client::GatewayClient;
        use crate::model::ModelBindings;
        use crate::observe::NullObserver;
        use crate::parser::Section;
        use crate::store::StoreRef;

        let worker = Section {
            name: "Worker".to_string(),
            level: 3,
            blocks: vec![Block::Prose {
                text: "irrelevant".to_string(),
                loop_capable: true,
            }],
            children: Vec::new(),
            items: Vec::new(),
        };
        let items: Vec<String> = (0..5).map(|i| i.to_string()).collect();
        let store = StoreRef::memory();
        let bindings = ToolBindings::default();
        let models = ModelBindings::default();
        let analysis = crate::execute::ToolAnalysis::default();
        let shared_tools = SharedTools::default();
        let client: Option<GatewayClient> = None;
        let observer = NullObserver;
        let ctx = FanoutContext {
            args: "",
            store: &store,
            execution: "fanout-cap-test",
            observer: &observer,
            client: &client,
            debug: None,
            shared: None,
            bindings: &bindings,
            models: &models,
            analysis: &analysis,
            shared_tools: &shared_tools,
            max_tool_iterations: 24,
            fanout_concurrency: NonZeroUsize::new(8).expect("8 is non-zero"),
            max_fanout_items: NonZeroUsize::new(3).expect("3 is non-zero"),
            lua_memory_bytes: 64 * 1024 * 1024,
            lua_log_events: 1024,
            last_reply: None,
            when: "2026-08-08",
            parent_id: 1,
            section_count: 1,
        };

        let error = run_fanout_arms(&worker, &items, &ctx)
            .await
            .expect_err("a list longer than max_fanout_items must be rejected");
        assert!(
            error.to_string().contains("exceeding the maximum of 3"),
            "error must explain the item cap: {error}"
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
        let analysis = crate::execute::ToolAnalysis::default();
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
            analysis: &analysis,
            shared_tools: &shared_tools,
            max_tool_iterations: 24,
            fanout_concurrency: NonZeroUsize::new(8).expect("8 is non-zero"),
            max_fanout_items: NonZeroUsize::new(1024).expect("1024 is non-zero"),
            lua_memory_bytes: 64 * 1024 * 1024,
            lua_log_events: 1024,
            last_reply: None,
            when: "2026-08-08",
            parent_id: 1,
            section_count: 1,
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

    /// Records every observation's Display string, in order.
    #[derive(Default)]
    struct EventRecorder(std::sync::Mutex<Vec<String>>);

    impl Observer for EventRecorder {
        fn observe(&self, _execution: &str, _section: &str, event: Observation) {
            self.0
                .lock()
                .expect("recorder mutex is not poisoned")
                .push(event.to_string());
        }
    }

    impl EventRecorder {
        fn snapshot(&self) -> Vec<String> {
            self.0
                .lock()
                .expect("recorder mutex is not poisoned")
                .clone()
        }

        fn count(&self, label: &str) -> usize {
            self.snapshot()
                .iter()
                .filter(|e| e.as_str() == label)
                .count()
        }
    }

    fn lua_worker(source: &str) -> Section {
        let program = LuaProgram::compile(
            source,
            "test prologue",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            "fanout-terminal-test",
            &crate::observe::NullObserver,
            "Worker",
        )
        .expect("test Lua must compile");
        Section {
            name: "Worker".to_string(),
            level: 3,
            blocks: vec![Block::Lua(program)],
            children: Vec::new(),
            items: Vec::new(),
        }
    }

    #[expect(
        clippy::ref_option,
        reason = "FanoutContext.client borrows an Option<GatewayClient>, so the helper must too"
    )]
    fn terminal_ctx<'a>(
        observer: &'a dyn Observer,
        store: &'a StoreRef,
        bindings: &'a ToolBindings,
        models: &'a ModelBindings,
        analysis: &'a crate::execute::ToolAnalysis,
        shared_tools: &'a SharedTools,
        client: &'a Option<GatewayClient>,
    ) -> FanoutContext<'a> {
        FanoutContext {
            args: "",
            store,
            execution: "fanout-terminal-test",
            observer,
            client,
            debug: None,
            shared: None,
            bindings,
            models,
            analysis,
            shared_tools,
            max_tool_iterations: 24,
            fanout_concurrency: NonZeroUsize::new(4).expect("4 is non-zero"),
            max_fanout_items: NonZeroUsize::new(1024).expect("1024 is non-zero"),
            lua_memory_bytes: 64 * 1024 * 1024,
            lua_log_events: 1024,
            last_reply: None,
            when: "2026-08-08",
            parent_id: 1,
            section_count: 1,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn each_arm_emits_a_distinct_succeeded_terminal_event() {
        // FANOUT-004: every arm is finalized exactly once with a distinct
        // terminal event. Two arms whose prologue returns a value each emit one
        // `started` and one `succeeded`, and nothing else.
        let worker = lua_worker("return item");
        let items = vec!["a".to_string(), "b".to_string()];
        let store = StoreRef::memory();
        let bindings = ToolBindings::default();
        let models = ModelBindings::default();
        let analysis = crate::execute::ToolAnalysis::default();
        let shared_tools = SharedTools::default();
        let client: Option<GatewayClient> = None;
        let recorder = EventRecorder::default();
        let ctx = terminal_ctx(
            &recorder,
            &store,
            &bindings,
            &models,
            &analysis,
            &shared_tools,
            &client,
        );

        let results = run_fanout_arms(&worker, &items, &ctx)
            .await
            .expect("both arms must succeed");
        assert_eq!(results.len(), 2);
        assert_eq!(recorder.count("Fanout arm started"), 2);
        assert_eq!(
            recorder.count("Fanout arm succeeded"),
            2,
            "each arm emits one distinct succeeded event: {:?}",
            recorder.snapshot()
        );
        assert_eq!(recorder.count("Fanout arm failed"), 0);
        assert_eq!(recorder.count("Fanout arm cancelled"), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hard_failing_arm_emits_a_failed_terminal_event() {
        // FANOUT-004: a hard arm error emits a distinct `failed` terminal event,
        // never `succeeded`.
        let worker = lua_worker("error('boom')");
        let items = vec!["a".to_string()];
        let store = StoreRef::memory();
        let bindings = ToolBindings::default();
        let models = ModelBindings::default();
        let analysis = crate::execute::ToolAnalysis::default();
        let shared_tools = SharedTools::default();
        let client: Option<GatewayClient> = None;
        let recorder = EventRecorder::default();
        let ctx = terminal_ctx(
            &recorder,
            &store,
            &bindings,
            &models,
            &analysis,
            &shared_tools,
            &client,
        );

        run_fanout_arms(&worker, &items, &ctx)
            .await
            .expect_err("a hard arm error must fail the fanout");
        assert_eq!(
            recorder.count("Fanout arm failed"),
            1,
            "the failing arm emits one failed event: {:?}",
            recorder.snapshot()
        );
        assert_eq!(recorder.count("Fanout arm succeeded"), 0);
    }

    /// Signals a oneshot the first time it observes a Lua `log` event, so a test
    /// can learn deterministically that an arm has started running.
    struct SignalOnLog {
        tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    }

    impl Observer for SignalOnLog {
        fn observe(&self, _execution: &str, _section: &str, event: Observation) {
            if matches!(event, Observation::Lua(_))
                && let Some(tx) = self.tx.lock().expect("signal mutex").take()
            {
                let _ = tx.send(());
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_in_flight_fanout_arm_is_cancelled_cooperatively() {
        // PF-CANCEL-002 / FANOUT-003 (in-flight): a spawned arm carries an
        // explicit CancelHandle, so an arm spinning in synchronous Lua stops via
        // its OWN instruction hook when cancelled mid-flight. Without the
        // per-arm handle the arm could not be aborted (synchronous Lua cannot be
        // preempted) and the join drain would hang - so the timeout below is the
        // regression guard. Readiness is signaled explicitly (no sleeps).
        use crate::cancel::{self, CancelHandle};

        let worker = lua_worker("log('running')\nwhile true do end\nreturn item");
        let items = vec!["only".to_string()];
        let store = StoreRef::memory();
        let bindings = ToolBindings::default();
        let models = ModelBindings::default();
        let analysis = crate::execute::ToolAnalysis::default();
        let shared_tools = SharedTools::default();
        let client: Option<GatewayClient> = None;

        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let observer = SignalOnLog {
            tx: std::sync::Mutex::new(Some(ready_tx)),
        };
        let ctx = terminal_ctx(
            &observer,
            &store,
            &bindings,
            &models,
            &analysis,
            &shared_tools,
            &client,
        );

        let cancel = CancelHandle::new();
        let canceller = {
            let handle = cancel.clone();
            tokio::spawn(async move {
                // Cancel only once the arm has actually started spinning.
                let _ = ready_rx.await;
                handle.cancel();
            })
        };

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            cancel::scope(cancel, run_fanout_arms(&worker, &items, &ctx)),
        )
        .await
        .expect("the in-flight arm must cooperatively cancel, not hang the join drain");
        let error = result.expect_err("a cancelled fanout returns an error");
        assert!(
            matches!(error, crate::Error::Interrupted),
            "expected Interrupted, got {error}"
        );
        canceller.await.expect("the canceller task joins");
    }

    #[test]
    fn arm_finalizer_emits_cancelled_on_drop_unless_finished() {
        // FANOUT-004/006: the guard emits exactly one terminal event. Dropped
        // without finishing => cancelled; finished => only that event.
        let (tx, mut rx) = mpsc::channel::<(String, Observation)>(8);
        let proxy = Arc::new(ProxyObserver { tx });

        drop(ArmFinalizer::new(
            Arc::clone(&proxy),
            "exec".to_string(),
            "S".to_string(),
        ));
        let (_, event) = rx.try_recv().expect("a dropped finalizer emits an event");
        assert_eq!(event.to_string(), "Fanout arm cancelled");
        assert!(rx.try_recv().is_err(), "exactly one terminal event on drop");

        let mut finalizer =
            ArmFinalizer::new(Arc::clone(&proxy), "exec".to_string(), "S".to_string());
        finalizer.finish(detail::FANOUT_ARM_SUCCEEDED);
        drop(finalizer);
        let (_, event) = rx.try_recv().expect("finish emits its event");
        assert_eq!(event.to_string(), "Fanout arm succeeded");
        assert!(
            rx.try_recv().is_err(),
            "a finished finalizer does not also emit cancelled on drop"
        );
    }
}
