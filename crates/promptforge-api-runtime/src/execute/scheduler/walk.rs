//! The section walk: sections run in fall-through order, `var` rolls
//! forward across sections and jumps, every section entry takes the
//! chain's next entry id (the chain's hierarchical id extended by its
//! local entry counter), and a jump transfers control - a sibling move
//! within the chain's slice, or a descent into the jumper's child slice
//! with the parent position suspended on the chain's own position stack
//! until the child level exhausts. A prompt with H1 blocks runs them first
//! as section 0 (the `h1` module), and the root walk starts from that
//! pass's `var` hand-off as the same root chain; a prompt without H1
//! blocks starts the root walk directly.

use std::sync::Arc;

use promptforge_api_types::ids::ChainId;

use crate::execute::engine::{
    JumpTarget, home_without, resolve_jump_target, section_position, visible_sections,
};
use crate::execute::section_context::SectionContext;
use crate::fanout;
use crate::parser::{Block, Section};
use crate::{Error, Result};

use super::{Chain, ChainIndex, Counters, Scheduler, prompt_origin};

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
    /// Starts the root walk chain over `sections` when the prompt has no
    /// H1 blocks, seeded with an empty `var`, and enqueues it. The walk is
    /// the root chain `0`; its entry 0 stays reserved for the H1 pass the
    /// prompt does not have, so the first walked section is `0.1` exactly
    /// as on a prompt whose pass ran.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the run's chain count exceeds `u32`,
    /// or [`Error::Store`] when the backend refuses acquisition.
    pub(super) fn start_root_walk(&mut self, sections: &'a [Section]) -> Result<()> {
        // The pass the prompt does not have would have taken root entry 0
        // and started no child, so the walk continues from the counters
        // it would have left.
        let after_h1 = Counters {
            next_child: 0,
            next_entry: 1,
        };
        let root = self.start_chain(
            ChainId::root(),
            after_h1,
            self.ctx.clone(),
            sections,
            0,
            None,
            &serde_json::json!({}),
            0,
            None,
        )?;
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
    pub(super) fn install_root_slots(&mut self, root: ChainIndex) -> Result<()> {
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

    /// Hands out the chain's next section-entry id: its hierarchical id
    /// extended by the local entry counter, the value the entered section
    /// reads as `sys.id`.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when one chain has entered `u32::MAX`
    /// sections, which no reachable run does.
    fn next_entry_id(chain: &mut Chain<'a>) -> Result<String> {
        let index = chain.counters.next_entry;
        chain.counters.next_entry = index
            .checked_add(1)
            .ok_or(Error::internal("a chain's entry count cannot exceed u32"))?;
        Ok(chain.lineage.entry(index))
    }

    /// Enters the chain's next section and reports whether one was entered:
    /// constructs the frame with the chain's next entry id and its task id,
    /// seeded from the chain's `var` and client slots (and, on a spawned
    /// chain's first entry, its `item` and `sys.index` seeds). The pending
    /// Markdown buffer
    /// resets: a previous section's unconsumed prose never crosses the
    /// boundary. `Ok(false)` means the
    /// slice is exhausted and the chain ends.
    ///
    /// # Errors
    /// Returns the [`Error`] of frame construction, as documented on
    /// [`SectionContext::new`].
    fn enter_section(&mut self, id: ChainIndex) -> Result<bool> {
        let chain = &mut self.chains[id.index()];
        chain.pending_prose = None;
        if chain.h1.is_some() {
            // The H1 pass enters its frame exactly once: section 0 under
            // the prompt's title, through the same install path as any
            // section - and no SECTION_STARTED, the pass is not a walked
            // section. Its id is the root chain's entry 0.
            let section_id = Self::next_entry_id(chain)?;
            let frame = SectionContext::new_live_h1(&chain.ctx, chain.access()?, &section_id)?;
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
            let section_id = Self::next_entry_id(chain)?;
            let task = chain.task.clone();
            let frame = SectionContext::new_fanout_arm(
                &chain.ctx,
                chain.access()?,
                worker,
                &home,
                &section_id,
                &task,
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
        let section_id = Self::next_entry_id(chain)?;
        let task = chain.task.clone();
        // A spawned chain's first entry consumes its `item` and `sys.index`
        // seeds; every later entry, and every other chain's, has none.
        let seed = chain.seed.take().unwrap_or_default();
        let frame = SectionContext::new(
            &chain.ctx,
            chain.access()?,
            &slice[index],
            slice,
            &section_id,
            &task,
            &chain.var,
            seed,
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
        id: ChainIndex,
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
    fn pop_position(&mut self, id: ChainIndex) -> bool {
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
    pub(super) fn end_section(&mut self, id: ChainIndex) -> Result<()> {
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
    pub(super) fn apply_jump(&mut self, id: ChainIndex, heading: &str) -> Result<()> {
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
        id: ChainIndex,
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
