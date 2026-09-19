//! The fanout join tables and arm bookkeeping. A `fanout` request forks N
//! arm chains (one per collection member) interleaved by the driver: at
//! most the run's `max_fanout_concurrency` arms are active at once, each
//! arm runs the same walk machinery as any chain over the worker's blocks,
//! and the join state's preallocated per-index slots deliver the results
//! to the parent in collection order, never finish order. The fanout
//! failure semantics match the legacy engine: an empty collection errors
//! before any scheduling, a fatal arm error aborts the sibling arms (each
//! aborted arm's finalizer reports `FANOUT_ARM_CANCELLED`, so exactly one
//! terminal observation fires per arm), [`Error::ToolLoopExhausted`]
//! soft-degrades its arm to the incomplete stub, and two live arms of one
//! fanout touching the same store path with at least one write terminate
//! the whole run with the claims model's fatal determinism violation,
//! intercepted at the answer boundary so no author `pcall` can catch it.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use crate::client::GatewayClient;
use crate::execute::context::RunState;
use crate::execute::protocol::Answer;
use crate::execute::support::MAX_CALL_DEPTH;
use crate::fanout::ArmFinalizer;
use crate::lua::LuaFanoutResult;
use crate::observe::detail;
use crate::parser::Section;
use crate::store::Access;
use crate::{Error, Result, cancel, subst};

use super::{ChainId, Scheduler, prompt_origin};

/// Join-table key for a live fanout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct FanoutId(u32);

/// One live fanout's join state: the arms still running, the preallocated
/// per-index result slots (collection order, never finish order), the
/// parent chain blocked on the join, the concurrency-window accounting, and
/// the template every arm chain starts from.
pub(super) struct JoinState<'a> {
    /// Arms still running; at zero the parent resumes with the sequence.
    remaining: usize,
    /// One slot per collection index, so results land in collection order.
    results: Vec<Option<LuaFanoutResult>>,
    /// The chain that yielded the fanout, blocked until the join completes.
    pub(super) parent: ChainId,
    /// Arms currently active (unblocked or pending on I/O); bounded by
    /// `window`.
    active: usize,
    /// The next collection index to start when a window slot frees.
    next: usize,
    /// The converted collection members, indexed by arm.
    items: Vec<serde_json::Value>,
    /// At most this many arms active at once: the run's
    /// `max_fanout_concurrency`.
    window: usize,
    /// Everything an arm chain starts from, shared by every arm of the
    /// fanout.
    template: ArmTemplate<'a>,
}

/// The arm-chain construction inputs one fanout's arms share.
#[derive(Clone)]
struct ArmTemplate<'a> {
    /// The fanout caller's walk position: an arm's visible set derives from
    /// it (the caller's slice minus the caller, plus the caller's children,
    /// minus the worker, plus the worker's children).
    caller_slice: &'a [Section],
    /// The caller's index in `caller_slice`.
    caller_index: usize,
    /// The slice the worker was resolved from (the caller's own slice or
    /// the caller's children).
    worker_slice: &'a [Section],
    /// The worker's index in `worker_slice`.
    worker_index: usize,
    /// The fanout's run-context fork: the run's own observer and debug sink
    /// (the legacy proxies exist to cross the spawned-task boundary, which
    /// a chain never crosses) with a fresh turn counter, so arm turns count
    /// against the fanout's own cap.
    ctx: RunState,
    /// The fanout caller's access capability: each arm spawns its own
    /// capability from it at dispatch, so the spawn retires the caller's
    /// claims (the happens-before edge) and two live arms touching one
    /// path meet the claims model's conflict rule.
    access: Arc<Access>,
    /// The caller's `var` snapshot; each arm seeds from its own clone and
    /// its writes never reach the caller.
    var: serde_json::Value,
    /// The arms' call depth: the fanout caller's depth plus one.
    call_depth: usize,
    /// The caller's client snapshot: each arm starts from it, resolving one
    /// lazily when absent.
    client: Option<GatewayClient>,
    /// The run's cancellation handle, captured at dispatch and handed to
    /// each arm chain directly (the scheduler has no spawned arm tasks to
    /// carry one across).
    cancel: Option<cancel::CancelHandle>,
}

/// One arm chain's fanout state: where its result lands and what its
/// worker entry is seeded with.
pub(super) struct ArmState<'a> {
    /// The join this arm reports to.
    pub(super) fanout: FanoutId,
    /// The arm's 0-based collection index: its result slot and (plus one)
    /// its `sys.index`.
    pub(super) item_index: usize,
    /// The arm's collection member: the `item` global and `{{ item }}`
    /// substitution seed for the worker entry.
    pub(super) item: serde_json::Value,
    /// True while the worker is the chain's current section: the worker
    /// entry gets the arm seeds, and a control transfer out of the worker
    /// resolves over the arm's visible set. Cleared by the first jump.
    pub(super) at_worker: bool,
    /// The fanout caller's walk position, as on the template.
    pub(super) caller_slice: &'a [Section],
    /// The caller's index in `caller_slice`.
    pub(super) caller_index: usize,
    /// The resolved worker's home slice position.
    pub(super) worker_slice: &'a [Section],
    /// The worker's index in `worker_slice`.
    pub(super) worker_index: usize,
    /// The run's cancellation handle, installed as the task-local around
    /// each of the arm's steps, so cancellation reaches the arm's running
    /// Lua through its instruction hook exactly as on the driver task.
    pub(super) cancel: Option<cancel::CancelHandle>,
    /// The arm's terminal-observation guard: the driver finishes it with
    /// the arm's real outcome (succeeded, exhausted, or failed), and its
    /// drop reports `FANOUT_ARM_CANCELLED` - a sibling's fatal error
    /// aborting this arm, or the run's cancellation dropping the
    /// scheduler, both pass through that drop. Exactly one terminal event
    /// fires per arm (the legacy `ArmFinalizer` contract).
    pub(super) finalizer: ArmFinalizer,
}

impl<'a> Scheduler<'a> {
    /// Dispatches a `fanout` request: resolves the worker, creates the join
    /// state with its preallocated per-index result slots, and starts the
    /// first window of arm chains; the parent blocks until the join
    /// completes. Every dispatch failure - the depth cap, an empty
    /// collection, worker resolution - is the call's answer, resumed into
    /// the caller so an author `pcall` can catch it exactly as on the
    /// legacy callback path.
    pub(super) fn dispatch_fanout(
        &mut self,
        id: ChainId,
        worker: &str,
        items: &[serde_json::Value],
        var: &serde_json::Value,
    ) {
        match self.prepare_fanout(id, worker, items, var) {
            Ok(()) => {}
            Err(error) => {
                self.chains[id.index()].incoming = Some(Answer::Fanout(Err(error)));
                self.ready.push_back(id);
            }
        }
    }

    /// The fallible half of fanout dispatch: the depth cap checked against
    /// the caller's call-depth field (each arm runs one level deeper),
    /// the empty collection rejected before any scheduling, the worker
    /// resolved over the caller's visible set, and the join state and
    /// first window of arm chains created.
    fn prepare_fanout(
        &mut self,
        id: ChainId,
        worker_name: &str,
        items: &[serde_json::Value],
        var: &serde_json::Value,
    ) -> Result<()> {
        let chain = &self.chains[id.index()];
        let depth = chain.call_depth + 1;
        if depth > MAX_CALL_DEPTH {
            return Err(Error::Lua(format!(
                "fanout recursion exceeded cap of {MAX_CALL_DEPTH}"
            )));
        }
        // An empty collection runs zero arms; that is an authoring bug (a
        // list section that parsed empty, a wrong variable), not a valid
        // run.
        if items.is_empty() {
            return Err(Error::Lua(
                "fanout over an empty collection: no work is likely a bug".to_owned(),
            ));
        }
        // An at-worker arm's fanout resolves over the worker's visible set
        // (handled inside `resolve_chain_target`); the new arms in turn
        // treat the worker as their caller.
        //
        // H1 has no position in the top-level slice: the worker's own
        // position stands in as the caller's, so the arm's visible set
        // comes out as the worker's siblings plus its children either way.
        let h1_caller = chain.h1.is_some();
        let (caller_slice, caller_index) = match &chain.arm {
            _ if h1_caller => {
                let target = self.resolve_chain_target(id, worker_name)?;
                (target.slice, target.index)
            }
            Some(arm) if arm.at_worker => (arm.worker_slice, arm.worker_index),
            _ => (chain.slice, chain.index),
        };
        let ctx = chain.ctx.clone();
        let client = chain.client.clone();
        // The caller's capability: each arm spawns its own from it, so the
        // spawn is the happens-before edge that retires the caller's claims.
        let access = chain
            .access
            .clone()
            .ok_or(Error::internal("a live chain holds its access capability"))?;
        // `chain`'s arena borrow ends here; the resolution borrows the
        // prompt tree, so the worker's slice outlives it.
        let target = self.resolve_chain_target(id, worker_name)?;
        let worker = &target.slice[target.index];
        if worker.prologue().is_none() && worker.epilog().is_none() && !worker.items().is_empty() {
            return Err(Error::Lua(format!(
                "section `{}` is a list section, not a worker template",
                worker.name()
            )));
        }
        let fanout_id = FanoutId(self.next_fanout);
        self.next_fanout += 1;
        self.joins.insert(
            fanout_id,
            JoinState {
                remaining: items.len(),
                results: vec![None; items.len()],
                parent: id,
                active: 0,
                next: 0,
                items: items.to_vec(),
                window: ctx.limits().fanout_concurrency().get(),
                template: ArmTemplate {
                    caller_slice,
                    caller_index,
                    worker_slice: target.slice,
                    worker_index: target.index,
                    // Arms report through the run's own observer and debug
                    // sink directly - the legacy proxies exist to cross the
                    // spawned-task boundary, which a chain never crosses -
                    // while the fanout's turn counter stays fresh, so arm
                    // turns count against the fanout's own cap.
                    ctx: ctx.with_effective_handles(
                        Arc::clone(ctx.observer()),
                        ctx.debug().cloned(),
                        Arc::new(AtomicU32::new(0)),
                    ),
                    access,
                    var: var.clone(),
                    call_depth: depth,
                    client,
                    cancel: cancel::current(),
                },
            },
        );
        // A mid-refill failure (the run's chain count exceeding the bound)
        // must not propagate with the join live and a partial window
        // enqueued: the caller resumes with this error as the fanout's
        // answer, and a late arm completion against the live join would
        // resume the parent a second time. Tear the fanout down instead -
        // the join goes and the started arms abort, each finalizer drop
        // reporting FANOUT_ARM_CANCELLED exactly as on fail_fanout's path -
        // and leave the parent's answer to the caller.
        if let Err(error) = self.refill_fanout(fanout_id) {
            self.joins.remove(&fanout_id);
            for arm in self.arm_chains_of(fanout_id) {
                self.abort_subtree(arm);
            }
            return Err(error);
        }
        Ok(())
    }

    /// Starts arm chains for one fanout while a window slot is free and
    /// items remain, enqueuing each on the ready queue. Each arm is a chain
    /// over the worker alone (a singleton slice); a jump out of the worker
    /// retargets the arm's walk.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the join is not live or the run's
    /// chain count exceeds `u32`, or [`Error::Store`] when the backend
    /// refuses an arm's acquisition.
    fn refill_fanout(&mut self, fanout: FanoutId) -> Result<()> {
        loop {
            let (index, item, template) = {
                let Some(join) = self.joins.get_mut(&fanout) else {
                    return Err(Error::internal("a window refill implies a live join"));
                };
                if join.next >= join.items.len() || join.active >= join.window {
                    return Ok(());
                }
                let index = join.next;
                join.next += 1;
                join.active += 1;
                (index, join.items[index].clone(), join.template.clone())
            };
            let worker_slice = template.worker_slice;
            let worker = &worker_slice[template.worker_index];
            // Arm creation is the dispatch boundary, so it carries the
            // arm's STARTED observation, exactly as the legacy arm task's
            // start did; the finalizer guards the exactly-once terminal
            // event from here on.
            template.ctx.observer().observe(
                template.ctx.execution(),
                worker.name(),
                detail::FANOUT_ARM_STARTED,
            );
            let arm = ArmState {
                fanout,
                item_index: index,
                item,
                at_worker: true,
                caller_slice: template.caller_slice,
                caller_index: template.caller_index,
                worker_slice,
                worker_index: template.worker_index,
                cancel: template.cancel.clone(),
                finalizer: ArmFinalizer::new(
                    Arc::clone(template.ctx.observer()),
                    template.ctx.execution().to_owned(),
                    worker.name().to_owned(),
                ),
            };
            let chain = self.start_chain(
                template.ctx.clone(),
                std::slice::from_ref(worker),
                0,
                None,
                &template.var,
                template.call_depth,
                Some(arm),
            )?;
            // The arm is a new concurrent thread of execution: its
            // capability spawns from the fanout caller's, retiring the
            // caller's claims (the happens-before edge), and drops with
            // the chain so a finished arm's claims never linger into the
            // join's merge. The arm's origin is the worker section's.
            let origin = prompt_origin(template.ctx.prompt(), worker.name(), worker.blocks());
            let access = template.access.spawn(origin).map_err(Error::Store)?;
            self.chains[chain.index()].access = Some(Arc::new(access));
            // The arm inherits the caller's client slot: an
            // already-resolved client is shared, an unresolved one stays
            // lazy.
            self.chains[chain.index()]
                .client
                .clone_from(&template.client);
            self.ready.push_back(chain);
        }
    }

    /// Applies one arm chain's end to its join, finishing the arm's
    /// terminal observation with its real outcome: a success writes the
    /// arm's preallocated slot (so results land in collection order) and
    /// refills the window; the last arm's landing resumes the parent with
    /// the packed sequence. [`Error::ToolLoopExhausted`] soft-degrades the
    /// arm to the incomplete stub, so one stuck arm cannot kill sibling
    /// evidence. Any other arm error is fatal: it fails the join and
    /// aborts the sibling arms.
    pub(super) fn complete_arm(&mut self, mut arm: ArmState<'a>, outcome: Result<String>) {
        /// How the join moves on one arm's end.
        enum ArmEnd {
            /// The slot is written and arms remain: refill the window.
            Continue,
            /// The last arm landed: pack the sequence for the parent.
            Complete,
            /// A fatal arm error: fail the fanout and abort the siblings.
            Fail(Error),
        }
        let end = {
            let Some(join) = self.joins.get_mut(&arm.fanout) else {
                // The join already failed on a sibling's fatal error and was
                // removed; this arm's outcome is discarded with it, and the
                // arm's drop reports the cancelled terminal event.
                return;
            };
            join.active -= 1;
            match outcome {
                Ok(text) => {
                    arm.finalizer.finish(detail::FANOUT_ARM_SUCCEEDED);
                    join.results[arm.item_index] = Some(LuaFanoutResult::success(arm.item, text));
                    join.remaining -= 1;
                    if join.remaining == 0 {
                        ArmEnd::Complete
                    } else {
                        ArmEnd::Continue
                    }
                }
                // One stuck arm must not kill sibling evidence facets.
                Err(Error::ToolLoopExhausted) => {
                    let stub = format!(
                        "## {}\n\nUNKNOWN\n\n(section incomplete: tool loop exhausted)",
                        subst::render_item(&arm.item)
                    );
                    arm.finalizer.finish(detail::FANOUT_ARM_EXHAUSTED);
                    join.results[arm.item_index] =
                        Some(LuaFanoutResult::exhausted_stub(arm.item, stub));
                    join.remaining -= 1;
                    if join.remaining == 0 {
                        ArmEnd::Complete
                    } else {
                        ArmEnd::Continue
                    }
                }
                Err(error) => {
                    arm.finalizer.finish(detail::FANOUT_ARM_FAILED);
                    ArmEnd::Fail(error)
                }
            }
        };
        match end {
            ArmEnd::Continue => {
                if let Err(error) = self.refill_fanout(arm.fanout) {
                    self.fail_fanout(arm.fanout, error);
                }
            }
            ArmEnd::Complete => {
                let Some(join) = self.joins.remove(&arm.fanout) else {
                    return;
                };
                // Every slot is Some here: `remaining` reached zero, so
                // every arm wrote its slot. The `ok_or_else` keeps that
                // invariant guarded, mirroring the legacy driver's check.
                let results = join
                    .results
                    .into_iter()
                    .enumerate()
                    .map(|(index, slot)| {
                        slot.ok_or_else(|| {
                            Error::Lua(format!(
                                "fanout arm {} finished without a result",
                                index + 1
                            ))
                        })
                    })
                    .collect();
                self.chains[join.parent.index()].incoming = Some(Answer::Fanout(results));
                self.ready.push_back(join.parent);
            }
            ArmEnd::Fail(error) => self.fail_fanout(arm.fanout, error),
        }
    }

    /// Fails one fanout's join: the sibling arms still alive are aborted
    /// (the legacy `JoinSet::abort_all` port - an aborted arm's frame drops
    /// unarmed and its finalizer reports `FANOUT_ARM_CANCELLED`), the
    /// parent resumes with the error, and the join is removed. Items never
    /// dispatched stay unstarted: with the join gone, no refill can create
    /// their arms.
    fn fail_fanout(&mut self, fanout: FanoutId, error: Error) {
        let Some(join) = self.joins.remove(&fanout) else {
            return;
        };
        for sibling in self.arm_chains_of(fanout) {
            self.abort_subtree(sibling);
        }
        self.chains[join.parent.index()].incoming = Some(Answer::Fanout(Err(error)));
        self.ready.push_back(join.parent);
    }

    /// The arena ids of one fanout's live arm chains. An arm whose chain
    /// already finished is absent: `finish` took its arm state, so only
    /// arms still running, suspended, or blocked carry it.
    pub(super) fn arm_chains_of(&self, fanout: FanoutId) -> Vec<ChainId> {
        // The arena is u32-bounded at insertion (`start_chain`), so the
        // index conversion cannot fail.
        self.chains
            .iter()
            .enumerate()
            .filter(|(_, chain)| chain.arm.as_ref().is_some_and(|arm| arm.fanout == fanout))
            .filter_map(|(index, _)| u32::try_from(index).ok().map(ChainId))
            .collect()
    }
}
