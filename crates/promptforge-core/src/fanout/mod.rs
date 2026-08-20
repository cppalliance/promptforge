//! Explicit fanout: map a worker section over a collection of members.
//!
//! A section's Lua calls `fanout(worker, collection)` to run the worker
//! template once per collection member. The collection is any Lua table: the
//! array part (`1..=#t`) iterates in order first, then the hash part in
//! undefined order. An array member arrives as the arm's `item` value as
//! itself; a hash member arrives as a pair table (`item.key` / `item.value`).
//! A list section's pre-parsed items feed in through `list_from_section`:
//! `fanout("### Worker", list_from_section("### List"))`. Arms execute
//! concurrently on a [`tokio::task::JoinSet`]; each gets a fresh [`SectionVm`]
//! with `item` and `sys.index` injected and `var` cloned in from the caller
//! (arm writes never reach the caller), and runs the same engine the section
//! walk drives: the shared VM setup, the `model:infer` hook, and the ordered
//! block walk (every Lua and prose block in order, the conversation and reply
//! rolling forward, the tool scope rebuilt per prose block, a gateway client
//! created from the environment on first use when the arm was handed none).
//! Arms run with the full control surface: `execute`, `fanout`, and
//! `list_from_section` resolve against the worker's visible set (the set the
//! worker was resolved from, minus the worker, plus its children), and `jump`
//! drives a child walk whose reply becomes the arm's text. Recursion depth
//! accumulates across the fanout boundary - each arm runs one level deeper
//! than its caller - so `MAX_EXECUTE_DEPTH` bounds `execute`/`fanout` nesting
//! uniformly. The invoker receives an ordered Lua
//! table of structured arm results (`.text`, `.ok`, `.item`, `.exhausted`),
//! with `.item` carrying the member value back. An empty collection is an
//! [`Error::Lua`] before any scheduling: no work is likely a bug. Fatal arm
//! errors abort siblings; [`Error::ToolLoopExhausted`] soft-degrades to an
//! incomplete stub. All arms share the run's store: two arms of one fanout
//! writing the same path is a hard write-write race error (which aborts
//! siblings like any fatal arm error), while `append` stays legal with
//! unspecified order.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64};

use mlua::{Lua, LuaSerdeExt, Value};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::cancel;
use crate::client::GatewayClient;
use crate::debug::DebugCapture;
use crate::execute::RunLimits;
use crate::lua::{LuaFanoutResult, LuaProgram, ToolBindings};
use crate::model::ModelBindings;
use crate::observe::{Observation, Observer};
use crate::parser::Section;
use crate::store::StoreRef;
use crate::tools::SharedTools;
use crate::{Error, Result};

/// Parses a section heading like `"### Name"` into an exact `(level, name)`
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
            "section heading must include ### markers, got bare name: {stripped}"
        )));
    }
    // The `#` run is ASCII, so a byte slice at `level` is a valid boundary.
    let rest = &stripped[level..];
    // Checked before the whitespace gate: a marker-only heading (`###`) has
    // no name to parse whether or not whitespace followed the markers.
    let name = rest.trim();
    if name.is_empty() {
        return Err(Error::Lua(format!(
            "section heading has no name: {stripped}"
        )));
    }
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return Err(Error::Lua(format!(
            "section heading must have whitespace after the {} markers: {stripped}",
            "#".repeat(level)
        )));
    }
    Ok((level, name.to_owned()))
}

/// Resolves a heading string like `"### Name"` against a caller's visible
/// sections, returning the single matching section.
///
/// The heading is parsed into an exact `(level, name)` address; a section
/// matches only when BOTH its level and name are equal. Zero matches and more
/// than one match are both rejected, so an ambiguous or level-mismatched
/// heading never resolves to an arbitrary first hit.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading is malformed (see
/// [`parse_heading_address`]), when no visible section matches the exact
/// address, or when more than one matches. The error message lists the
/// visible sections and nothing else, so the error channel cannot leak the
/// rest of the document's structure.
pub(crate) fn resolve_sibling<'a>(heading: &str, visible: &'a [Section]) -> Result<&'a Section> {
    let (level, name) = parse_heading_address(heading)?;

    let mut matches = visible
        .iter()
        .filter(|section| usize::from(section.level) == level && section.name == name);
    let Some(found) = matches.next() else {
        let available: Vec<String> = visible
            .iter()
            .map(|s| format!("{} {}", "#".repeat(s.level.into()), s.name))
            .collect();
        return Err(Error::Lua(format!(
            "section heading `{}` not found; available sections: {}",
            heading.trim(),
            available.join(", ")
        )));
    };
    if matches.next().is_some() {
        return Err(Error::Lua(format!(
            "section heading `{}` is ambiguous; more than one visible section matches {} {name}",
            heading.trim(),
            "#".repeat(level)
        )));
    }
    Ok(found)
}

/// Converts fanout's collection argument into the JSON members that cross
/// into the arms, one value at a time.
///
/// The array part (`1..=#t`) iterates in order first, then the hash part in
/// undefined order. Array members convert as themselves; hash members convert
/// to `{"key": k, "value": v}` pair tables so no information is lost. Each
/// member converts individually through the same serde bridge that seeds
/// `var`, because whole-table serde cannot represent mixed tables.
///
/// # Errors
/// Returns [`Error::Lua`] when the value is not a table (the message points
/// at `list_from_section` for the list-section case), when a member is a
/// function, userdata, or thread (the error names the member's index), or
/// when a hash key is not a string, number, or boolean.
pub(crate) fn collection_to_items(lua: &Lua, collection: &Value) -> Result<Vec<serde_json::Value>> {
    let Value::Table(table) = collection else {
        return Err(Error::Lua(
            "fanout's second parameter is a collection; for a list section use list_from_section(heading)".to_owned(),
        ));
    };
    let mut items = Vec::new();
    let border = table.raw_len();
    for index in 1..=border {
        let member = table.raw_get::<Value>(index).map_err(Error::lua)?;
        items.push(member_to_json(lua, member, &index.to_string())?);
    }
    for pair in table.pairs::<Value, Value>() {
        let (key, member) = pair.map_err(Error::lua)?;
        // The array part was already emitted above, in order.
        if let Value::Integer(index) = &key
            && usize::try_from(*index).is_ok_and(|index| (1..=border).contains(&index))
        {
            continue;
        }
        // Each scalar key converts to its JSON form and its diagnostic label
        // in one match; non-scalar keys are rejected here, so no later code
        // path can meet one.
        let (key_json, key_label) = match &key {
            Value::String(s) => {
                let s = s.to_str().map_err(Error::lua)?;
                (serde_json::Value::String(s.to_owned()), s.to_owned())
            }
            Value::Integer(i) => (serde_json::Value::from(*i), i.to_string()),
            Value::Number(n) => (
                serde_json::Number::from_f64(*n)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| {
                        Error::Lua("fanout collection key is not a finite number".to_owned())
                    })?,
                n.to_string(),
            ),
            Value::Boolean(b) => (serde_json::Value::Bool(*b), b.to_string()),
            other => {
                return Err(Error::Lua(format!(
                    "fanout collection key must be a string, number, or boolean, got {}",
                    other.type_name()
                )));
            }
        };
        let value_json = member_to_json(lua, member, &key_label)?;
        items.push(json!({ "key": key_json, "value": value_json }));
    }
    Ok(items)
}

/// Converts one collection member to JSON through the serde bridge.
///
/// Functions, userdata, and threads cannot serialize, so they are rejected at
/// the call boundary with an error naming the member's index rather than the
/// bridge's type error.
fn member_to_json(lua: &Lua, member: Value, index: &str) -> Result<serde_json::Value> {
    match &member {
        Value::Function(_) | Value::UserData(_) | Value::Thread(_) => Err(Error::Lua(format!(
            "fanout collection member at index {index} is a {}; members must be data",
            member.type_name()
        ))),
        _ => lua.from_value(member).map_err(Error::lua),
    }
}

/// Everything a fanout needs from the invoker's context.
pub(crate) struct FanoutContext<'a> {
    pub args: &'a str,
    pub store: &'a StoreRef,
    pub execution: &'a str,
    pub observer: &'a dyn Observer,
    pub client: &'a Option<GatewayClient>,
    pub debug: Option<&'a dyn DebugCapture>,
    /// The shared library every arm replays as its first chunk; an empty
    /// compiled chunk when the prompt declares no `lua shared` library.
    pub shared: &'a LuaProgram,
    pub bindings: &'a ToolBindings,
    pub models: &'a ModelBindings,
    pub analysis: &'a crate::execute::ToolAnalysis,
    pub shared_tools: &'a SharedTools,
    pub max_tool_iterations: usize,
    /// The run's resource limits: the concurrency window is read from it
    /// here, each arm reads its Lua ceilings from it, and a lazily
    /// created gateway client inherits its HTTP limits.
    pub limits: RunLimits,
    pub last_reply: Option<&'a str>,
    pub when: &'a str,
    /// The run-global execution-id counter every arm takes its `sys.id`
    /// from; a fanout shares it without resetting (unlike `turns`).
    pub ids: &'a Arc<AtomicU64>,
    /// Total H2 section count in the top-level prompt.
    pub section_count: usize,
    /// The worker's home slice - the set it was resolved from, minus the
    /// worker. Each arm's control globals (`execute`, `fanout`,
    /// `list_from_section`, and a jump's target) derive their resolution set
    /// from it as the home slice plus the worker's children.
    pub home: &'a [Section],
    /// The fanout caller's execute depth. Each arm runs one level deeper, so
    /// recursion accounting accumulates across the fanout boundary.
    pub execute_depth: usize,
    /// The fanout caller's `var`, snapshotted at the call site: each arm
    /// seeds from its own clone, and arm writes never reach the caller.
    pub var: &'a serde_json::Value,
}

/// Runs the worker section template once per item, concurrently.
///
/// Returns the ordered structured results from each arm (collection order,
/// not finish order).
///
/// # Errors
/// Returns [`Error::Lua`] when `items` is empty: no work is likely a bug, so
/// the fanout is rejected before any scheduling. Fatal arm errors abort
/// siblings; tool-loop exhaustion soft-degrades.
pub(crate) async fn run_fanout_arms(
    worker: &Section,
    items: &[serde_json::Value],
    ctx: &FanoutContext<'_>,
) -> Result<Vec<LuaFanoutResult>> {
    // An empty collection runs zero arms; that is an authoring bug (a list
    // section that parsed empty, a wrong variable), not a valid run.
    if items.is_empty() {
        return Err(Error::Lua(
            "fanout over an empty collection: no work is likely a bug".to_owned(),
        ));
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
    let (observe_tx, observe_rx) = mpsc::channel::<(String, Observation)>(SIDE_CHANNEL_CAPACITY);
    let (debug_tx, debug_rx) = mpsc::channel::<DebugMsg>(SIDE_CHANNEL_CAPACITY);
    let mut side_channels = SideChannels {
        observe_rx,
        debug_rx,
    };
    let proxy_observer = Arc::new(ProxyObserver { tx: observe_tx });
    let proxy_debug = ctx.debug.map(|_| {
        Arc::new(ProxyDebugCapture {
            tx: debug_tx.clone(),
        }) as Arc<dyn DebugCapture>
    });

    // The inputs every arm shares, built once and carried by Arc into each
    // arm's payload across the spawn boundary, so an N-item fanout pays one
    // inputs construction rather than N deep clones.
    let inputs = Arc::new(ArmInputs::from_context(
        ctx,
        worker,
        &turns,
        Arc::clone(&proxy_observer),
        proxy_debug.clone(),
        arm_cancel.clone(),
    ));

    let mut join_set: JoinSet<Result<(usize, LuaFanoutResult)>> = JoinSet::new();
    let mut replies: Vec<Option<LuaFanoutResult>> = vec![None; items.len()];

    // Spawns arm `index`, pairing the shared inputs with that arm's own item.
    // Concurrency is bounded by only ever having `ArmWindow`-approved arms
    // resident in the `JoinSet`.
    let spawn_arm = |index: usize, join_set: &mut JoinSet<Result<(usize, LuaFanoutResult)>>| {
        let payload = ArmPayload {
            inputs: Arc::clone(&inputs),
            item: items[index].clone(),
            index,
        };
        join_set.spawn(run_one_arm(payload));
    };

    // Spawns every arm the window currently allows: seeds the initial window,
    // then refills it as arms complete.
    let fill_window =
        |window: &mut ArmWindow, join_set: &mut JoinSet<Result<(usize, LuaFanoutResult)>>| {
            while let Some(index) = window.take_next() {
                spawn_arm(index, join_set);
            }
        };

    // At most `fanout_concurrency` arms are resident at once: seed the initial
    // window, then schedule the next queued item whenever one completes.
    let mut window = ArmWindow::new(items.len(), ctx.limits.fanout_concurrency());
    fill_window(&mut window, &mut join_set);

    // Drop the unused sender clone so the debug channel can close when arms finish.
    drop(debug_tx);

    loop {
        tokio::select! {
            biased;
            () = cancel::wait_cancelled() => {
                abort_fanout_arms(&mut join_set, ctx, &mut side_channels).await;
                return Err(Error::Interrupted);
            }
            Some((section, event)) = side_channels.observe_rx.recv() => {
                ctx.observer.observe(ctx.execution, &section, event);
            }
            Some(msg) = side_channels.debug_rx.recv() => {
                forward_debug(ctx, msg);
            }
            joined = join_set.join_next() => {
                match joined {
                    None => break,
                    Some(Ok(Ok((index, reply)))) => {
                        replies[index] = Some(reply);
                        window.complete_one();
                        fill_window(&mut window, &mut join_set);
                    }
                    Some(Ok(Err(error))) => {
                        abort_fanout_arms(&mut join_set, ctx, &mut side_channels).await;
                        return Err(error);
                    }
                    // A cancelled JoinError cannot reach this loop: the only
                    // abort path (`abort_fanout_arms`) drains the JoinSet
                    // before its caller returns.
                    Some(Err(join_error)) => {
                        abort_fanout_arms(&mut join_set, ctx, &mut side_channels).await;
                        // Keep the structured JoinError as the error source; it is
                        // only stringified at the Lua callback boundary.
                        return Err(Error::FanoutArmJoin(join_error));
                    }
                }
            }
        }
    }

    side_channels.drain(ctx);

    // Every slot is Some here: each arm-failure path returns early, and
    // `ArmWindow` dispatches each index exactly once, so a drained JoinSet
    // means every arm replied. The `ok_or_else` keeps that invariant guarded.
    let ordered = replies
        .into_iter()
        .enumerate()
        .map(|(index, reply)| {
            reply.ok_or_else(|| {
                Error::Lua(format!("fanout arm {} finished without a reply", index + 1))
            })
        })
        .collect::<Result<Vec<_>>>()?;
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

/// Forwards one arm debug event to the run's capture sink, when one is set.
fn forward_debug(ctx: &FanoutContext<'_>, msg: DebugMsg) {
    if let Some(capture) = ctx.debug {
        capture.on_event(ctx.execution, &msg.section, msg.turn_index, msg.event);
    }
}

/// The receive halves of the two arm side channels, bundled so the abort and
/// drain paths thread one value rather than restating the same pair.
struct SideChannels {
    observe_rx: mpsc::Receiver<(String, Observation)>,
    debug_rx: mpsc::Receiver<DebugMsg>,
}

impl SideChannels {
    /// Forwards every event the arms left buffered, draining both channels.
    fn drain(&mut self, ctx: &FanoutContext<'_>) {
        while let Ok((section, event)) = self.observe_rx.try_recv() {
            ctx.observer.observe(ctx.execution, &section, event);
        }
        while let Ok(msg) = self.debug_rx.try_recv() {
            forward_debug(ctx, msg);
        }
    }
}

/// Aborts every outstanding arm, drains the `JoinSet`, and flushes the side
/// channels so events the arms already buffered still reach the run's sinks.
async fn abort_fanout_arms(
    join_set: &mut JoinSet<Result<(usize, LuaFanoutResult)>>,
    ctx: &FanoutContext<'_>,
    side_channels: &mut SideChannels,
) {
    join_set.abort_all();
    while join_set.join_next().await.is_some() {}
    side_channels.drain(ctx);
}

mod arm;
mod proxies;

use arm::{ArmInputs, ArmPayload, run_one_arm};
use proxies::{DebugMsg, ProxyDebugCapture, ProxyObserver, SIDE_CHANNEL_CAPACITY};

#[cfg(test)]
mod tests;
