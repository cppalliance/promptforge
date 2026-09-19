//! The frame's three constructors: one per arrival kind. Each absorbs the
//! VM construction and setup preamble for its kind of section entry - a
//! walked section, the live H1 pass (section 0), a fanout arm - and hands
//! back a live [`SectionContext`] whose `Drop` is the teardown boundary.
//! The setup half (host injection, host APIs, the control surface, the
//! shared replay, the captured alias bindings) is shared; only the seed,
//! the `sys` extras, and the `list_from_section` visible set differ.

use std::sync::Arc;

use promptforge_api_types::ids::{ChainId, TaskId};

use crate::execute::context::RunState;
use crate::execute::engine::{list_items_from_visible, visible_sections};
use crate::execute::section_vm::{VmSeed, setup_section_vm};
use crate::execute::support::{now_rfc3339_checked, sys_json};
use crate::lua::SectionVm;
use crate::observe::detail;
use crate::parser::Section;
use crate::store::Access;
use crate::{Error, Result};

use super::{SectionContext, TaskSeed};

impl SectionContext {
    /// Constructs the frame for one walked section and runs its setup
    /// preamble: the `sys` JSON, the section-started observation, VM
    /// construction and limits, the control surface (the `jump` and
    /// `list_from_section` callbacks resolved over the section's visible
    /// set, plus the coroutine yield shims for the suspending calls), the
    /// shared setup half (host injection, host APIs, the shared replay, the
    /// captured alias bindings).
    ///
    /// `siblings` is the caller's own walk slice, from which the section's
    /// visible set (its siblings minus itself, plus its direct children) is
    /// built for the `list_from_section` callback. `section_id` is the
    /// section's `sys.id`: the entering chain's hierarchical id extended
    /// by its local entry counter, allocated by the scheduler; `task_id`
    /// is the entering chain's `sys.taskid`. `var` is the walk's current
    /// clipboard, seeded into the section's VM. `seed` carries a spawned
    /// chain's `item` and `sys.index` on its first entry and is empty on
    /// every other entry.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed. A VM
    /// construction or limits failure propagates bare, before any teardown
    /// observation exists; a setup failure tears the fresh VM down first, so
    /// the teardown boundary still fires exactly once on that path.
    #[expect(
        clippy::too_many_arguments,
        reason = "the walk frame keeps its context, capability, section, visible slice, entry id, task id, var seed, and task seed explicit and linear"
    )]
    pub(crate) fn new(
        ctx: &RunState,
        access: &Arc<Access>,
        section: &Section,
        siblings: &[Section],
        section_id: &str,
        task_id: &TaskId,
        var: &serde_json::Value,
        seed: TaskSeed,
    ) -> Result<Self> {
        let mut sys = ctx.sys_json(section_id, task_id, section.name())?;
        // A spawned chain's `sys.index` is the spawn's own value, verbatim;
        // absent otherwise, so a walked section reading `sys.index` raises
        // the sealed-sys unknown-field error exactly as before.
        if let Some(index) = seed.index {
            sys["index"] = serde_json::Value::from(index);
        }
        ctx.observer()
            .observe(ctx.execution(), section.name(), detail::SECTION_STARTED);
        let mut vm = SectionVm::new_for_section(
            ctx.nonce(),
            &ctx.tool_set(),
            &ctx.model_set(),
            ctx.execution(),
            ctx.observer().as_ref(),
            section.name(),
        )?;
        // A limits failure propagates bare: no teardown runs here, so no
        // LUA_TEARDOWN_* observation fires on this path.
        vm.apply_lua_limits(
            ctx.limits().lua_memory().get(),
            ctx.limits().lua_logs().get(),
        )?;
        // The `list_from_section` callback resolves over the section's
        // visible set; the suspending calls (`call`, `fanout`,
        // `models.infer`) are the yield shims the setup half installs.
        let visible = visible_sections(siblings, section);
        let list_callback = move |heading: String| list_items_from_visible(&heading, &visible);
        // The setup half of the section lifecycle - host injection, host
        // APIs, the control surface, the shared replay, and the captured
        // alias bindings - is shared with the fanout arm; only the seed, the
        // `sys` extras, and the callback's visible set are the walk's own.
        let setup = ctx.vm_setup(
            &sys,
            VmSeed {
                var: Some(var),
                item: seed.item.as_ref(),
            },
            access,
            section.name(),
        );
        // Setup runs on the bare VM so a failure tears it down here: the
        // frame does not exist yet, so its `Drop` cannot own this path.
        if let Err(error) = setup_section_vm(&mut vm, &setup, list_callback) {
            vm.teardown(ctx.observer().as_ref(), section.name());
            return Err(error);
        }
        Ok(Self {
            vm: Some(vm),
            name: section.name().to_owned(),
            execution: ctx.execution().to_owned(),
            completed: false,
            sys,
            var: var.clone(),
            item: seed.item,
            counts: None,
            observer: Arc::clone(ctx.observer()),
            debug: ctx.debug().cloned(),
            turns: Arc::clone(ctx.turns()),
        })
    }

    /// Constructs the frame for the H1 pass - section 0 - through the same
    /// install path as any walked section: the `sys` JSON (`section_id`,
    /// the root chain's entry 0, under the prompt's title, stamped with
    /// its own `now` because the walk's `when` does not exist yet), VM
    /// construction over the run's shared sets, limits, and the shared
    /// setup half (host injection, host APIs, the control surface, the
    /// coroutine shims, the shared replay, the captured alias bindings).
    ///
    /// H1's only deltas from a walked section: no `SECTION_STARTED`
    /// observation (the pass is not a walked section), an empty `var` seed
    /// (it runs first and is never re-entered), and a `list_from_section`
    /// visible set spanning the whole top-level slice - section 0 excludes
    /// nothing and has no children.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed. A VM
    /// construction or limits failure propagates bare, before any teardown
    /// observation exists; a setup failure tears the fresh VM down first, so
    /// the teardown boundary still fires exactly once on that path.
    pub(crate) fn new_live_h1(
        ctx: &RunState,
        access: &Arc<Access>,
        section_id: &str,
    ) -> Result<Self> {
        let title = ctx.prompt().title();
        let now = now_rfc3339_checked()?;
        // The pass is the root chain, and the root chain is task `0`.
        let root_task = TaskId::from(ChainId::root());
        let sys = sys_json(
            &now,
            &now,
            section_id,
            &root_task.to_string(),
            title,
            ctx.execution(),
            ctx.prompt().sections().len(),
        );
        let mut vm = SectionVm::new_for_section(
            ctx.nonce(),
            &ctx.tool_set(),
            &ctx.model_set(),
            ctx.execution(),
            ctx.observer().as_ref(),
            title,
        )?;
        // A limits failure propagates bare: no teardown runs here, so no
        // LUA_TEARDOWN_* observation fires on this path.
        vm.apply_lua_limits(
            ctx.limits().lua_memory().get(),
            ctx.limits().lua_logs().get(),
        )?;
        // H1's visible set is the whole top-level slice: section 0
        // excludes nothing and has no children.
        let visible = ctx.prompt().sections().to_vec();
        let list_callback = move |heading: String| list_items_from_visible(&heading, &visible);
        // H1's one privilege: `argv` installs writable, so the repair
        // pattern can assign it; the executor reads the value back at the
        // freeze (see the scheduler's H1-to-walk handoff).
        let mut setup = ctx.vm_setup(&sys, VmSeed::default(), access, title);
        setup.argv_writable = true;
        // Setup runs on the bare VM so a failure tears it down here: the
        // frame does not exist yet, so its `Drop` cannot own this path.
        if let Err(error) = setup_section_vm(&mut vm, &setup, list_callback) {
            vm.teardown(ctx.observer().as_ref(), title);
            return Err(error);
        }
        Ok(Self {
            vm: Some(vm),
            name: title.to_owned(),
            execution: ctx.execution().to_owned(),
            completed: false,
            sys,
            var: serde_json::json!({}),
            item: None,
            counts: None,
            observer: Arc::clone(ctx.observer()),
            debug: ctx.debug().cloned(),
            turns: Arc::clone(ctx.turns()),
        })
    }

    /// Constructs the frame for one fanout arm and runs its setup preamble:
    /// VM construction and limits, the `sys` JSON carrying the arm chain's
    /// entry id as `id` and its 1-based per-fanout `index`, the control
    /// surface (the `list_from_section` callback resolved over the worker's
    /// visible set: its home slice plus its children; plus the yield
    /// shims), and the shared setup half.
    ///
    /// The seed is the fanout's own: the collection `item`, the arm's
    /// spawned access capability (its claims-model identity), and the
    /// caller's cloned `var`; `task_id` is the fanout caller's task, which
    /// the arm reports as its `sys.taskid`. The
    /// effective reporting handles
    /// are the fanout's too: the run's own observer and debug sink with the
    /// fanout's fresh turn counter arrive through the context's fanout fork,
    /// so the arm's nested `call`/`fanout` chains report through them as
    /// well.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed. A VM
    /// construction failure propagates bare - no VM exists to tear down. A
    /// limits, `sys`, or setup failure tears the fresh VM down once here:
    /// the chain owns the run phase's teardown boundary, so the
    /// construction phase keeps its own and every path tears down exactly
    /// once.
    #[expect(
        clippy::too_many_arguments,
        reason = "the arm frame keeps its context, capability, worker, visible set, entry id, task id, index, item, and var seed explicit and linear"
    )]
    pub(crate) fn new_fanout_arm(
        ctx: &RunState,
        access: &Arc<Access>,
        worker: &Section,
        home: &[Section],
        section_id: &str,
        task_id: &TaskId,
        index: usize,
        item: serde_json::Value,
        var: &serde_json::Value,
    ) -> Result<Self> {
        let mut vm = SectionVm::new_for_section(
            ctx.nonce(),
            &ctx.tool_set(),
            &ctx.model_set(),
            ctx.execution(),
            ctx.observer().as_ref(),
            worker.name(),
        )?;
        // The limits install and the `sys` build are the construction
        // phase's fallible steps once the VM exists; a failure tears the
        // fresh VM down once here, matching the single teardown the arm's
        // epilogue owns for the run phase.
        let sys = match vm
            .apply_lua_limits(
                ctx.limits().lua_memory().get(),
                ctx.limits().lua_logs().get(),
            )
            .map_err(Error::from)
            .and_then(|()| {
                let mut sys = ctx.sys_json(section_id, task_id, worker.name())?;
                // The arm's own sys extra: its 1-based position within this
                // fanout. Absent outside a fanout, so a walked section
                // reading `sys.index` raises the sealed-sys unknown-field
                // error; a nested fanout's arms restart at 1.
                sys["index"] = serde_json::Value::from(index + 1);
                Ok(sys)
            }) {
            Ok(sys) => sys,
            Err(error) => {
                vm.teardown(ctx.observer().as_ref(), worker.name());
                return Err(error);
            }
        };
        let item = Some(item);
        // The `list_from_section` callback resolves over the worker's
        // visible set (its home slice plus its children); the suspending
        // calls are the yield shims the setup half installs.
        let visible = visible_sections(home, worker);
        let list_callback = move |heading: String| list_items_from_visible(&heading, &visible);
        // The setup half is shared with the walk; only the seed, the `sys`
        // extra, and the callback's visible set are the arm's own.
        let setup = ctx.vm_setup(
            &sys,
            VmSeed {
                var: Some(var),
                item: item.as_ref(),
            },
            access,
            worker.name(),
        );
        // Setup runs on the bare VM so a failure tears it down here: the
        // frame does not exist yet, so its `Drop` cannot own this path.
        if let Err(error) = setup_section_vm(&mut vm, &setup, list_callback) {
            vm.teardown(ctx.observer().as_ref(), worker.name());
            return Err(error);
        }
        Ok(Self {
            vm: Some(vm),
            name: worker.name().to_owned(),
            execution: ctx.execution().to_owned(),
            completed: false,
            sys,
            var: var.clone(),
            item,
            counts: None,
            observer: Arc::clone(ctx.observer()),
            debug: ctx.debug().cloned(),
            turns: Arc::clone(ctx.turns()),
        })
    }
}
