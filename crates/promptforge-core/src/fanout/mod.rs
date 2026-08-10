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

use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::cancel;
use crate::client::GatewayClient;
use crate::debug::DebugCapture;
use crate::lua::{LuaFanoutResult, LuaProgram, ToolBindings};
use crate::model::ModelBindings;
use crate::observe::{Observation, Observer};
use crate::parser::Section;
use crate::store::StoreRef;
use crate::tools::SharedTools;
use crate::{Error, Result};

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

mod arm;
mod proxies;

use arm::{ArmPayload, run_one_arm};
use proxies::{DebugMsg, ProxyDebugCapture, ProxyObserver, SIDE_CHANNEL_CAPACITY};

#[cfg(test)]
mod tests;
