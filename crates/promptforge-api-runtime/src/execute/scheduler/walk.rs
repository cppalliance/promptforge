//! The section walk: sections run in fall-through order, `var` rolls
//! forward across sections and jumps, every section entry takes the next
//! run-global id, and a jump transfers control - a sibling move within the
//! chain's slice, or a descent into the jumper's child slice with the
//! parent position suspended on the chain's own position stack until the
//! child level exhausts. A prompt with H1 blocks runs them first as
//! section 0: the driver loop's first chain, under the walk's rules with
//! three deltas - the frame keeps id 0, a scalar return short-circuits the
//! run, and a Lua failure is the prompt's failed hard gate, mapped to
//! [`Error::RequirementsUnmet`] - with the root walk starting from the H1
//! `var` hand-off.

use std::sync::Arc;

use crate::execute::engine::{
    JumpTarget, home_without, resolve_jump_target, section_position, visible_sections,
};
use crate::execute::section_context::SectionContext;
use crate::execute::support::{GENERIC_COMPLETION, next_id, now_rfc3339_checked};
use crate::fanout;
use crate::parser::{Block, Section};
use crate::{Error, Result};

use super::{Chain, ChainId, Scheduler, prompt_origin};

/// A heading resolved against a chain's visible set: the slice the walk or
/// a contained chain continues on, the target's index in it, and whether
/// the target is a direct child of the current section (a descent).
pub(super) struct ChainTarget<'a> {
    /// The slice the walk or chain continues on.
    pub(super) slice: &'a [Section],
    /// The target's index in `slice`.
    pub(super) index: usize,
    /// True when the target is a direct child of the current section.
    pub(super) child: bool,
}

/// Resolves `heading` against an at-worker arm's visible set: the fanout
/// caller's visible set minus the worker, plus the worker's children - the
/// set the legacy arm's control globals resolve over, built with the same
/// helpers so resolution and its error listings match exactly.
///
/// A sibling-level target walks its own prompt slice from its index: the
/// worker's home slice when it lives there, else the caller's children or
/// the caller's slice. The resolved `(level, name)` pair is unique across
/// the visible set (an ambiguous resolve already failed), so at most one
/// slice contains it. One legacy edge narrows here: the legacy arm walks
/// its materialized home slice (the caller's slice minus the caller and
/// the worker, concatenated with the caller's children), so a target that
/// precedes the worker never falls through back into it, and a
/// level-matched member of the caller's sibling slice walks on into the
/// caller's children; the scheduler walks the target's own prompt slice
/// instead.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading is malformed, matches no
/// visible section, or matches more than one (see
/// [`fanout::resolve_sibling`]); [`Error::Internal`] when a resolved
/// target is absent from every home slice (an invariant violation).
fn resolve_arm_target<'a>(
    caller_slice: &'a [Section],
    caller_index: usize,
    worker_slice: &'a [Section],
    worker_index: usize,
    heading: &str,
) -> Result<ChainTarget<'a>> {
    let caller = &caller_slice[caller_index];
    let worker = &worker_slice[worker_index];
    let mut visible = home_without(&visible_sections(caller_slice, caller), worker);
    visible.extend(worker.children().iter().cloned());
    let target = fanout::resolve_sibling(heading, &visible)?;
    if let Some(index) = section_position(worker.children(), target) {
        return Ok(ChainTarget {
            slice: worker.children(),
            index,
            child: true,
        });
    }
    for slice in [worker_slice, caller.children(), caller_slice] {
        if let Some(index) = section_position(slice, target) {
            return Ok(ChainTarget {
                slice,
                index,
                child: false,
            });
        }
    }
    Err(Error::internal(
        "a resolved arm target is absent from its home slices",
    ))
}

impl<'a> Scheduler<'a> {
    /// Starts the root walk chain over `sections`, seeded with the H1
    /// pass's hand-off `var` (empty when the drive has no H1 phase), and
    /// enqueues it.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the run's chain count exceeds `u32`,
    /// or [`Error::Store`] when the backend refuses acquisition.
    pub(super) fn start_root_walk(
        &mut self,
        sections: &'a [Section],
        var: &serde_json::Value,
    ) -> Result<()> {
        let root = self.start_chain(self.ctx.clone(), sections, 0, None, var, 0, None)?;
        self.install_root_slots(root)?;
        self.ready.push_back(root);
        Ok(())
    }

    /// Seeds a fresh root walk chain's slots: its own access capability -
    /// the walk is its own serial thread of execution, and a fresh acquire
    /// (the H1 pass's identity ended with its chain) means nothing the pass
    /// touched can false-conflict with the walk - and its client slot from
    /// the run's configured client, as the legacy walk's slot is seeded
    /// from run()'s client: a prose block before any infer must use it
    /// rather than fall back to building an environment client.
    ///
    /// # Errors
    /// Returns [`Error::Store`] when the backend refuses acquisition.
    fn install_root_slots(&mut self, root: ChainId) -> Result<()> {
        // The walk capability serves every section in turn, so its label
        // is the prompt's own; the line is where the walk starts.
        let prompt = self.ctx.prompt();
        let blocks: &[Block] = prompt
            .sections()
            .first()
            .map_or(&[], |section| section.blocks());
        let origin = prompt_origin(prompt, prompt.title(), blocks);
        let access = self.ctx.vfs().acquire(origin).map_err(Error::Store)?;
        self.chains[root.index()].access = Some(Arc::new(access));
        self.chains[root.index()].client = self.client.ready().cloned();
        Ok(())
    }

    /// Starts the H1 pass as the driver loop's first chain: the prompt's
    /// H1 blocks under its title - section 0 - driven through the same
    /// coroutine machinery as any section.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the run's chain count exceeds `u32`,
    /// or [`Error::Store`] when the backend refuses acquisition.
    pub(super) fn start_live_h1(&mut self) -> Result<ChainId> {
        let id = ChainId(
            u32::try_from(self.chains.len())
                .map_err(|_| Error::internal("a run's chain count cannot exceed u32"))?,
        );
        // The pass owns its client slot, seeded from the run's configured
        // client, exactly as the legacy pass seeds its own.
        let client = self.client.ready().cloned();
        // The live H1 pass runs under the prompt's title, from its first
        // compiled H1 chunk.
        let origin = prompt_origin(
            self.ctx.prompt(),
            self.ctx.prompt().title(),
            self.ctx.prompt().h1_blocks(),
        );
        let access = self.ctx.vfs().acquire(origin).map_err(Error::Store)?;
        self.chains.push(Chain {
            ctx: self.ctx.clone(),
            access: Some(Arc::new(access)),
            frame: None,
            slice: &[],
            index: 0,
            positions: Vec::new(),
            block: 0,
            coroutine: None,
            incoming: None,
            pending_prose: None,
            var: serde_json::json!({}),
            call_depth: 0,
            client,
            parent: None,
            arm: None,
            h1: Some(self.ctx.prompt().h1_blocks()),
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
    /// [`Error::TimestampFormat`] when the walk's `when` fails to format,
    /// [`Error::Store`] when the backend refuses the walk's acquisition,
    /// or [`Error::Internal`] when the chain holds no frame.
    pub(super) fn end_live_h1(
        &mut self,
        id: ChainId,
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
        let sections = self.ctx.prompt().sections();
        if sections.is_empty() {
            *root_result = Some(Ok(GENERIC_COMPLETION.to_owned()));
            return Ok(());
        }
        // The H1-to-walk handoff: the walk's context takes its live `when`
        // and the frozen `argv`; H1's prompt-wide records already landed in
        // the shared sets the views read.
        let when = now_rfc3339_checked()?;
        let walk_ctx = self.ctx.with_walk_state(&when, argv);
        let root = self.start_chain(walk_ctx, sections, start, None, &var, 0, None)?;
        self.install_root_slots(root)?;
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
        id: ChainId,
        heading: &str,
        root_result: &mut Option<Result<String>>,
    ) -> Result<()> {
        let sections = self.ctx.prompt().sections();
        let target = fanout::resolve_sibling(heading, sections)?;
        let start = section_position(sections, target).ok_or(Error::internal(
            "a resolved H1 jump target is absent from the top-level slice",
        ))?;
        self.end_live_h1(id, root_result, start)
    }

    /// Enters the chain's next section and reports whether one was entered:
    /// constructs the frame with the next run-global id, seeded from
    /// the chain's `var` and client slots. The pending Markdown buffer
    /// resets: a previous section's unconsumed prose never crosses the
    /// boundary. `Ok(false)` means the
    /// slice is exhausted and the chain ends.
    ///
    /// # Errors
    /// Returns the [`Error`] of frame construction, as documented on
    /// [`SectionContext::new`].
    fn enter_section(&mut self, id: ChainId) -> Result<bool> {
        let chain = &mut self.chains[id.index()];
        chain.pending_prose = None;
        if chain.h1.is_some() {
            // The H1 pass enters its frame exactly once: section 0 under
            // the prompt's title, through the same install path as any
            // section - and no SECTION_STARTED, the pass is not a walked
            // section.
            let frame = SectionContext::new_live_h1(&chain.ctx, chain.access()?)?;
            chain.frame = Some(frame);
            chain.block = 0;
            return Ok(true);
        }
        // A fanout arm's first entry constructs the worker frame with the
        // arm's own seeds: the collection item, the store-write scope, the
        // caller's cloned `var`, and the worker's visible set for the
        // `list_from_section` callback. Later entries of the arm's walk
        // (after a jump) are plain sections on the walk path below.
        if let Some(arm) = &chain.arm
            && arm.at_worker
        {
            let (worker_slice, worker_index) = (arm.worker_slice, arm.worker_index);
            let (caller_slice, caller_index) = (arm.caller_slice, arm.caller_index);
            let (item_index, item) = (arm.item_index, arm.item.clone());
            let worker = &worker_slice[worker_index];
            let caller = &caller_slice[caller_index];
            let home = home_without(&visible_sections(caller_slice, caller), worker);
            let frame = SectionContext::new_fanout_arm(
                &chain.ctx,
                chain.access()?,
                worker,
                &home,
                item_index,
                item,
                &chain.var,
            )?;
            chain.frame = Some(frame);
            chain.block = 0;
            return Ok(true);
        }
        let index = chain.index;
        if index >= chain.slice.len() {
            return Ok(false);
        }
        // `slice` borrows the prompt tree, not the arena, so the frame
        // construction can borrow the chain's own context and slots.
        let slice = chain.slice;
        let frame = SectionContext::new(
            &chain.ctx,
            chain.access()?,
            &slice[index],
            slice,
            next_id(chain.ctx.ids()),
            &chain.var,
        )?;
        chain.frame = Some(frame);
        chain.block = 0;
        Ok(true)
    }

    /// Enters the chain's next section and requeues it, or finishes the
    /// chain when its slice is exhausted.
    ///
    /// # Errors
    /// Returns the [`Error`] of frame construction, as documented on
    /// [`SectionContext::new`].
    pub(super) fn advance_entry(
        &mut self,
        id: ChainId,
        root_result: &mut Option<Result<String>>,
    ) -> Result<()> {
        if self.enter_section(id)? {
            self.ready.push_back(id);
        } else if self.pop_position(id) {
            // A jump-started child level exhausted: the parent walk resumes
            // after the jumper.
            self.ready.push_back(id);
        } else {
            // The walk ran off the slice's last section: the chain ends.
            self.finish(id, Ok(None), root_result);
        }
        Ok(())
    }

    /// Resumes a jump-suspended parent position when a child level
    /// exhausts, returning `false` when the chain holds no suspended
    /// position - meaning its own root slice exhausted and the chain ends.
    /// The `var` slot needs no handling: the child walk shared
    /// it, so it already carries the child level's last value.
    fn pop_position(&mut self, id: ChainId) -> bool {
        let chain = &mut self.chains[id.index()];
        let Some((slice, jumper)) = chain.positions.pop() else {
            return false;
        };
        chain.slice = slice;
        chain.index = jumper + 1;
        true
    }

    /// Falls the chain through at its section's end: the section's final
    /// `var` replaces the chain's clipboard, read back while the VM is
    /// live; the frame's drop is
    /// the teardown boundary, firing `SECTION_FINISHED` for this completed
    /// section; then the walk advances to the next section.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the final `var` read-back fails (the
    /// frame drops unarmed, as on the legacy path), or
    /// [`Error::Internal`] when the chain holds no frame.
    pub(super) fn end_section(&mut self, id: ChainId) -> Result<()> {
        let chain = &mut self.chains[id.index()];
        let Some(mut frame) = chain.frame.take() else {
            return Err(Error::internal("a section end implies a live frame"));
        };
        chain.var = frame.read_var()?;
        frame.mark_completed();
        drop(frame);
        if let Some(arm) = &mut chain.arm {
            // The worker's own entry is complete; the arm's walk continues
            // (or ends) as plain sections, exactly as after a jump out.
            arm.at_worker = false;
        }
        chain.index += 1;
        Ok(())
    }

    /// Applies a jump's control transfer: closes the jumper's frame as
    /// completed (the final `var`
    /// rolled forward; the armed drop firing `SECTION_FINISHED`, a jump
    /// being a completion), resolves the heading against the jumper's
    /// visible set, and moves the walk. A sibling target sets the index
    /// within the target's slice; a child target pushes the
    /// current position onto the chain's position stack and descends into
    /// the jumper's child slice from the target.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the `var` read-back fails (the
    /// frame drops unarmed, as on the legacy path) or when the heading
    /// matches no visible section or more than one - the jumper's frame has
    /// already closed as completed, exactly as the legacy walk resolves
    /// after the jumper's teardown.
    pub(super) fn apply_jump(&mut self, id: ChainId, heading: &str) -> Result<()> {
        let (slice, index) = {
            let chain = &mut self.chains[id.index()];
            let Some(mut frame) = chain.frame.take() else {
                return Err(Error::internal("a jump implies a live frame"));
            };
            chain.var = frame.read_var()?;
            frame.mark_completed();
            drop(frame);
            (chain.slice, chain.index)
        };
        let target = self.resolve_chain_target(id, heading)?;
        let chain = &mut self.chains[id.index()];
        if let Some(arm) = &mut chain.arm {
            // The worker's own entry is left behind by the transfer; later
            // entries of the arm's walk are plain sections.
            arm.at_worker = false;
        }
        if target.child {
            chain.positions.push((slice, index));
        }
        chain.slice = target.slice;
        chain.index = target.index;
        Ok(())
    }

    /// Resolves `heading` against the chain's current section's visible set
    /// and returns the slice the walk or a contained chain continues on:
    /// the jumper's child slice for a direct child, the target's own slice
    /// otherwise.
    ///
    /// For an arm chain still at its worker, the visible set is the fanout
    /// caller's visible set minus the worker, plus the worker's children -
    /// the set the legacy arm's control globals resolve over.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the heading is malformed, matches no
    /// visible section, or matches more than one (see
    /// [`fanout::resolve_sibling`]).
    pub(super) fn resolve_chain_target(
        &self,
        id: ChainId,
        heading: &str,
    ) -> Result<ChainTarget<'a>> {
        let chain = &self.chains[id.index()];
        if chain.h1.is_some() {
            // H1 is section 0: its visible set is the whole top-level
            // slice - it excludes nothing and has no children, so every
            // target is a flat index into that slice.
            let sections = self.ctx.prompt().sections();
            let target = fanout::resolve_sibling(heading, sections)?;
            let index = section_position(sections, target).ok_or(Error::internal(
                "a resolved H1 target is absent from the top-level slice",
            ))?;
            return Ok(ChainTarget {
                slice: sections,
                index,
                child: false,
            });
        }
        if let Some(arm) = &chain.arm
            && arm.at_worker
        {
            let (caller_slice, caller_index) = (arm.caller_slice, arm.caller_index);
            let (worker_slice, worker_index) = (arm.worker_slice, arm.worker_index);
            return resolve_arm_target(
                caller_slice,
                caller_index,
                worker_slice,
                worker_index,
                heading,
            );
        }
        let slice = chain.slice;
        let index = chain.index;
        // `slice` borrows the prompt tree, not the arena, so the jumper
        // outlives the chain borrow above.
        let jumper = &slice[index];
        match resolve_jump_target(heading, slice, jumper)? {
            JumpTarget::Child(child) => Ok(ChainTarget {
                slice: jumper.children(),
                index: child,
                child: true,
            }),
            JumpTarget::Sibling(sibling) => Ok(ChainTarget {
                slice,
                index: sibling,
                child: false,
            }),
        }
    }
}
