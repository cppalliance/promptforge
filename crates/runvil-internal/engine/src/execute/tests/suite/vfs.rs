//! The executor's `VfsRef` contract with its caller: an end-to-end run
//! over the prepared handle, and the seed-run-extract round trip a caller
//! drives with no real files - seed the declared input through the run's
//! handle, prepare and run, extract the declared output, and charge a
//! missing output to the prompt's promise as an explicit contract error.
//! Then the run-bounded scope: a run's `Done`, or its drop before `Done`,
//! ends its scope however long the caller holds the store views it was
//! handed.

use std::sync::Arc;

use crate::execute::RunResult;
use crate::execute::run::{Effect, EffectId, Run, Step};
use crate::execute::tests::context::{parse, test_context};
use crate::parser::Prompt;
use crate::test_support::perform_locally;
use crate::test_support::scenario::{EXECUTION, Scenario};
use crate::test_support::temp_root::TempRoot;
use runvil_parser::test_support::{parse_ok, prompt};
use runvil_types::ids::Provenance;
use runvil_vfs::{Access, Origin, RealBackend, VfsError, VfsRef};

const ROUND_TRIP_PROMPT: &str = concat!(
    "---\n",
    "name: round-trip\n",
    "description: Seed, run, extract\n",
    "runvil: 0\n",
    "input:\n",
    "  path: paper.md\n",
    "  description: The input paper\n",
    "output:\n",
    "  path: report.md\n",
    "  description: The output report\n",
    "---\n\n",
    "# Review\n\n",
    "## Summarize\n\n",
    "```lua\n",
    "local paper = store.read('paper.md')\n",
    "store.write('report.md', 'report on: ' .. paper)\n",
    "return 'done'\n",
    "```\n",
);

const MISSING_OUTPUT: &str = concat!(
    "---\n",
    "name: missing-output\n",
    "description: Never writes its promised report\n",
    "runvil: 0\n",
    "input:\n",
    "  path: paper.md\n",
    "  description: The input paper\n",
    "output:\n",
    "  path: report.md\n",
    "  description: The output report\n",
    "---\n\n",
    "# Review\n\n",
    "## Summarize\n\n",
    "```lua\n",
    "local paper = store.read('paper.md')\n",
    "return 'read: ' .. paper\n",
    "```\n",
);

/// The store view derived from a fresh access on the run's handle: the
/// caller's seeding and extraction half of the seed-run-extract round trip.
fn fresh_store_view(vfs: &VfsRef, origin: &str) -> Access {
    let access = vfs
        .acquire(Origin::new(origin))
        .expect("the prepared backend acquires");
    runvil_vfs::detail::store_view(&access).expect("the handle declares a store")
}

/// The test's extraction rule: a declared output the run did not leave
/// behind is an explicit contract error naming the prompt's promise, never
/// a bare not-found.
fn extract_declared_output(store: &Access, prompt: &Prompt) -> Result<String, String> {
    let output = prompt
        .frontmatter()
        .output()
        .expect("the fixture declares an output");
    store
        .read_string(output.path())
        .map_err(|error| match error {
            VfsError::NotFound { .. } => format!(
                "the prompt promised output '{}' ({}) but the run left no such file",
                output.path(),
                output.description()
            ),
            other => format!("extracting the declared output failed: {other}"),
        })
}

/// Seeds the prompt's declared input through the handle the run will take.
/// The seeding access drops here - its scope ends - so the run's own
/// scope never meets the caller's.
fn seed_declared_input(vfs: &VfsRef, prompt: &Prompt, contents: &str) {
    let input = prompt
        .frontmatter()
        .input()
        .expect("the fixture declares an input");
    fresh_store_view(vfs, "seed_declared_input")
        .write(input.path(), contents.as_bytes())
        .expect("the declared input seeds");
}

/// Seeds `source`'s declared input with `contents` and runs it over the
/// seeded handle through the prepare, which keeps the handle as given:
/// the test's seed-run-extract sequence is seed through the handle,
/// prepare and run, extract through the handle. Returns the run's text
/// and the handle.
#[track_caller]
fn seeded_run(source: &str, name: &str, contents: &str) -> (String, VfsRef) {
    let vfs = VfsRef::default();
    seed_declared_input(&vfs, &parse_ok(source), contents);
    let trace = Scenario::new(source).name(name).vfs(vfs.clone()).run();
    (trace.text().to_owned(), vfs)
}

#[test]
fn an_end_to_end_run_threads_one_vfs_ref_through_every_section() {
    // The store survives the context-clearing section transition: one
    // section's write is the next section's read, over the run's
    // prepared handle.
    let source = prompt(
        "",
        &[
            ("First", "store.write('handoff.txt', 'across the reset')"),
            ("Second", "return store.read('handoff.txt')"),
        ],
    );
    let trace = Scenario::new(source).name("vfs-e2e").run();
    assert_eq!(trace.text(), "across the reset");
    // Extraction after the run takes a fresh access: the run's identities
    // dropped with it, so nothing the run touched can conflict here.
    assert_eq!(
        trace
            .read("handoff.txt")
            .expect("the run's write persists on the handle"),
        "across the reset"
    );
}

#[test]
fn a_caller_seeds_and_extracts_through_the_prepared_handle_with_no_real_files() {
    let (text, vfs) = seeded_run(ROUND_TRIP_PROMPT, "vfs-round-trip", "the paper body");
    assert_eq!(text, "done");
    let store = fresh_store_view(&vfs, "round-trip extraction");
    let report = extract_declared_output(&store, &parse_ok(ROUND_TRIP_PROMPT))
        .expect("the run left its promised output");
    assert_eq!(report, "report on: the paper body");
}

#[test]
fn a_missing_declared_output_is_a_contract_error_naming_the_prompts_promise() {
    // The executor does not enforce the declaration; the run succeeds and
    // the caller's extraction is where the broken promise surfaces.
    let (text, vfs) = seeded_run(MISSING_OUTPUT, "vfs-missing-output", "the paper body");
    assert_eq!(text, "read: the paper body");
    let store = fresh_store_view(&vfs, "missing-output extraction");
    let error = extract_declared_output(&store, &parse_ok(MISSING_OUTPUT))
        .expect_err("the missing output is a contract error");
    assert!(
        error.contains("report.md"),
        "the error names the promised path: {error}"
    );
    assert!(
        error.contains("The output report"),
        "the error names the promise's description: {error}"
    );
}

#[test]
fn fanout_interleaving_is_invariant_across_memory_and_real_backends() {
    // The consistency rule: every store operation takes the leaf-yield path
    // uniformly, with no inline fast path, so a run's observable behavior
    // cannot depend on which backend serves the store mount. The same
    // fanout fixture (arm-scoped writes, a post-join glob, the ordered
    // merge) runs over the stock memory mount and over a `RealBackend`
    // rooted in a temp dir; the result and the stored contents must be
    // identical.
    const FANOUT_STORE_WRITES: &str =
        include_str!("../../../../tests/prompts/execution/fanout-store-writes.md");
    // Both arms drive the raw caller-supplied handle contract (no prepare
    // pass), so the caller's own backend serves the declared store in each.
    let memory = Scenario::new(FANOUT_STORE_WRITES)
        .name("vfs-invariance-memory")
        .vfs(VfsRef::default())
        .unprepared()
        .run();

    let temp = TempRoot::new("fanout_interleaving_is_invariant_across_memory_and_real_backends");
    let real_vfs = VfsRef::builder()
        .store(
            "/",
            RealBackend::rooted(temp.path()).expect("the temp dir roots the real backend"),
        )
        .build();
    let real = Scenario::new(FANOUT_STORE_WRITES)
        .name("vfs-invariance-real")
        .vfs(real_vfs)
        .unprepared()
        .run();

    assert_eq!(
        memory.text(),
        real.text(),
        "the run's result must not depend on the backend"
    );
    for path in ["arm-1.md", "arm-2.md", "merged.md"] {
        assert_eq!(
            memory.read(path).ok(),
            real.read(path).ok(),
            "stored contents at {path} must not depend on the backend"
        );
    }
    // The `RealBackend` really served the mount: the arm's write landed on
    // the real filesystem under the root.
    assert!(
        temp.path().join("arm-1.md").is_file(),
        "the real backend must persist the arm's write under its root"
    );
}

const HELD_STORE: &str = "---\nname: held-store\ndescription: d\nrunvil: 0\n---\n\n\
    # Title\n\n\
    ## Only\n\n\
    ```lua\n\
    store.write('kept.txt', 'from the run')\n\
    return store.read('kept.txt')\n\
    ```\n";

/// Starts the `HELD_STORE` run over `vfs`.
fn held_store_run(vfs: &VfsRef) -> Run {
    let ctx = test_context(EXECUTION).vfs(vfs.clone());
    Run::new(Arc::new(parse(HELD_STORE)), "", ctx)
}

/// Answers `effects` through the local performer, keeping each one in
/// `held` so the store views they carry outlive the answer.
fn answer_holding(
    run: &mut Run,
    effects: Vec<(EffectId, Provenance, Effect)>,
    held: &mut Vec<Effect>,
) {
    assert!(!effects.is_empty(), "every issued effect is answered");
    for (id, _, effect) in effects {
        let answer = perform_locally(&effect, &mut |effect| {
            panic!("the fixture issues no model round: {effect:?}")
        });
        run.resume(id, answer);
        held.push(effect);
    }
}

/// Drives the `HELD_STORE` run over `vfs` to `Done`, keeping every
/// effect, and returns the run itself too, so both the run and every
/// store view it handed out are still alive when the caller asserts.
fn drive_holding_effects(vfs: &VfsRef) -> (Run, RunResult, Vec<Effect>) {
    let mut run = held_store_run(vfs);
    let mut held = Vec::new();
    loop {
        match run.step() {
            Step::Done { result, .. } => return (run, result, held),
            Step::Pending { effects, .. } => answer_holding(&mut run, effects, &mut held),
        }
    }
}

#[test]
fn a_run_ends_its_scope_at_done_while_the_caller_still_holds_its_store_views()
-> Result<(), VfsError> {
    let vfs = VfsRef::default();
    let (run, result, held) = drive_holding_effects(&vfs);
    assert!(
        matches!(&result, RunResult::Ok(text) if text == "from the run"),
        "the run succeeds: {result:?}"
    );
    assert!(
        held.iter()
            .any(|effect| matches!(effect, Effect::Vfs { .. })),
        "the test driver holds the run's store views past Done: {held:?}"
    );
    // A fresh scope reads the run's write without a conflict: the run's
    // scope ended at Done, not when the run or the test driver's views dropped.
    let fresh = vfs.acquire(Origin::new("after done"))?;
    assert_eq!(fresh.read("/kept.txt")?, b"from the run");
    drop(held);
    drop(run);
    Ok(())
}

#[test]
fn an_operation_through_a_store_view_held_past_done_is_refused_and_changes_nothing()
-> Result<(), VfsError> {
    let vfs = VfsRef::default();
    let (run, _, held) = drive_holding_effects(&vfs);
    let access = match held.first() {
        Some(Effect::Vfs { access, .. }) => access,
        other => panic!("the run's first effect is its store write: {other:?}"),
    };
    match access.write("kept.txt", b"after the run") {
        Err(VfsError::ScopeEnded { path }) => {
            assert_eq!(path, "kept.txt", "the refusal names the logical path");
        }
        other => panic!("a write through a view held past Done is refused, got {other:?}"),
    }
    let fresh = vfs.acquire(Origin::new("after done"))?;
    assert_eq!(fresh.read("/kept.txt")?, b"from the run");
    drop(held);
    drop(run);
    Ok(())
}

#[test]
fn dropping_a_run_before_done_ends_its_scope_while_the_caller_still_holds_its_store_views()
-> Result<(), VfsError> {
    let vfs = VfsRef::default();
    let mut run = held_store_run(&vfs);
    let mut held = Vec::new();
    let effects = match run.step() {
        Step::Pending { effects, .. } => effects,
        other @ Step::Done { .. } => panic!("the run parks on its store write: {other:?}"),
    };
    answer_holding(&mut run, effects, &mut held);
    let effects = match run.step() {
        Step::Pending { effects, .. } => effects,
        other @ Step::Done { .. } => panic!("the run parks on its store read: {other:?}"),
    };
    held.extend(effects.into_iter().map(|(_, _, effect)| effect));
    drop(run);
    // A fresh scope reads the dropped run's write without a conflict,
    // though the test driver still holds every store view the run handed out.
    let fresh = vfs.acquire(Origin::new("after drop"))?;
    assert_eq!(fresh.read("/kept.txt")?, b"from the run");
    drop(held);
    Ok(())
}
