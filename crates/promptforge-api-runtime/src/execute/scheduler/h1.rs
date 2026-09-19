//! The live H1 pass: the prompt's H1 blocks run first as section 0, the
//! driver loop's first chain, under the walk's rules with three deltas -
//! the frame takes the root chain's entry 0, a scalar return
//! short-circuits the run, and a Lua failure is the prompt's failed hard
//! gate, mapped to [`Error::RequirementsUnmet`]. The pass and the walk
//! that follows it are the same root chain `0`: the hand-off starts the
//! walk from the pass's `var` and frozen `argv`, continues the pass's
//! child and entry counters, so the first walked section is `0.1` and a
//! child the pass started keeps its index, and hands the pass's tasks to
//! the walk as their owner.

use std::sync::Arc;

use promptforge_api_types::ids::{ChainId, TaskId};

use crate::execute::engine::section_position;
use crate::execute::support::GENERIC_COMPLETION;
use crate::fanout;
use crate::{Error, Result};

use super::{Chain, ChainIndex, Counters, Scheduler, SlicePath, prompt_origin};

impl Scheduler {
    /// Starts the H1 pass as the driver loop's first chain: the prompt's
    /// H1 blocks under its title - section 0 - driven through the same
    /// coroutine machinery as any section.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the run's chain count exceeds `u32`,
    /// or [`Error::Store`] when the backend refuses acquisition.
    pub(super) fn start_live_h1(&mut self) -> Result<ChainIndex> {
        let id = ChainIndex(
            u32::try_from(self.chains.len())
                .map_err(|_| Error::internal("a run's chain count cannot exceed u32"))?,
        );
        // The live H1 pass runs under the prompt's title, from its first
        // compiled H1 chunk.
        let origin = prompt_origin(
            self.ctx.prompt(),
            self.ctx.prompt().title(),
            self.ctx.prompt().h1_blocks(),
        );
        let access = self.ctx.vfs().acquire(origin).map_err(Error::Store)?;
        // The pass is the root chain: its one frame takes entry 0, and the
        // walk that follows continues its counters as the same chain.
        self.chains.push(Chain {
            lineage: ChainId::root(),
            counters: Counters::default(),
            task: TaskId::from(ChainId::root()),
            owner: None,
            seed: None,
            waiting_on: Vec::new(),
            awaiting: None,
            blocked: None,
            task_notices: Vec::new(),
            note: None,
            ctx: self.ctx.clone(),
            access: Some(Arc::new(access)),
            frame: None,
            slice: SlicePath::root(),
            index: 0,
            positions: Vec::new(),
            block: 0,
            coroutine: None,
            incoming: None,
            pending_prose: None,
            var: serde_json::json!({}),
            call_depth: 0,
            parent: None,
            advertised: None,
            h1: true,
        });
        Ok(id)
    }

    /// Ends the H1 pass at its fall-through: the final `var` read back
    /// while the VM is live, then the frame drops unarmed - the
    /// pass never arms completion, so `SECTION_FINISHED` never fires for
    /// it. The root walk then starts from the `var` hand-off at section
    /// `start` (0 on a fall-through, the resolved target on a jump out)
    /// under the walk's own context fork; with no sections the run's
    /// result is the shared generic completion.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the final `var` read-back fails or H1
    /// left `argv` as non-JSON data,
    /// [`Error::Store`] when the backend refuses the walk's acquisition,
    /// or [`Error::Internal`] when the chain holds no frame.
    pub(super) fn end_live_h1(
        &mut self,
        id: ChainIndex,
        root_result: &mut Option<Result<String>>,
        start: usize,
    ) -> Result<()> {
        let chain = &mut self.chains[id.index()];
        let Some(mut frame) = chain.frame.take() else {
            return Err(Error::internal("the H1 pass ends with a live frame"));
        };
        let var = frame.read_var()?;
        // The freeze: whatever `argv` H1 leaves behind - the derived parse
        // or its repair - is what every walked section inherits, frozen.
        let argv = frame.read_argv()?;
        drop(frame);
        // The pass's chain ends here: release its capability (and with it
        // the identity's claims) before the walk acquires its own.
        chain.access = None;
        // The walk is the same root chain as the pass, so it continues the
        // pass's counters: the pass took entry 0, the first walked section
        // takes entry 1, and a child the pass started keeps its index.
        let counters = chain.counters;
        if self.ctx.prompt().sections().is_empty() {
            // No walk follows, so the pass's end is the run's end: a task
            // the pass spawned and left live ends here under the same
            // rules as a finishing chain.
            *root_result = Some(self.settle_owned_tasks(id, Ok(GENERIC_COMPLETION.to_owned())));
            return Ok(());
        }
        // The H1-to-walk handoff: the walk's context takes the frozen
        // `argv`; H1's prompt-wide records already landed in the shared
        // sets the views read, and `when` is the run's own.
        let walk_ctx = self.ctx.with_walk_state(argv);
        let root = self.start_chain(
            ChainId::root(),
            counters,
            walk_ctx,
            SlicePath::root(),
            start,
            None,
            &var,
            0,
        )?;
        self.install_root_slots(root)?;
        // The walk is the pass's continuation, so the tasks the pass
        // spawned are the walk's from here: it waits on, cancels, or leaks
        // them exactly as if it had spawned them.
        self.reassign_tasks(id, root);
        self.ready.push_back(root);
        Ok(())
    }

    /// Ends the H1 pass on a jump out: the heading resolves against the
    /// top-level sections (H1's visible set - section 0 excludes nothing
    /// and has no children), then the pass ends and the root walk starts
    /// at the target.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the heading is malformed, matches no
    /// top-level section, or matches more than one; the pass's own ending
    /// can fail as [`end_live_h1`](Self::end_live_h1) documents.
    pub(super) fn end_live_h1_at_jump(
        &mut self,
        id: ChainIndex,
        heading: &str,
        root_result: &mut Option<Result<String>>,
    ) -> Result<()> {
        let prompt = self.prompt();
        let sections = prompt.sections();
        let target = fanout::resolve_sibling(heading, sections)?;
        let start = section_position(sections, target).ok_or(Error::internal(
            "a resolved H1 jump target is absent from the top-level slice",
        ))?;
        self.end_live_h1(id, root_result, start)
    }
}
