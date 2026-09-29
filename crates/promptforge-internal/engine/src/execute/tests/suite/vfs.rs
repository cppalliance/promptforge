//! The executor's `VfsRef` host contract: an end-to-end run over the
//! prepared handle, and the seed-run-extract round trip a production host
//! like papergate drives with no real files - prepare, seed the
//! declared input through the run's handle, run, extract the declared
//! output, and charge a missing output to the prompt's promise as an
//! explicit contract error. Then the run-bounded scope: a run's `Done`,
//! or its drop before `Done`, ends its scope however long the host holds
//! the store views it was handed.

use crate::execute::RunResult;
use crate::execute::run::{Effect, EffectId, Run, Step};
use crate::parser::Prompt;
use promptforge_types::ids::Provenance;
use promptforge_vfs::{Access, HostBackend, Origin, VfsError, VfsRef};

use super::super::context::{EXECUTION, parse, test_context};
use super::super::serial_driver::perform_locally;
use super::support::Recorder;
use super::support::{RunOptions, drive, parse_execution_fixture, prepare_run, run_fixture};
use std::sync::Arc;

const ROUND_TRIP: &str = concat!(
    "---\n",
    "name: papergate-round-trip\n",
    "description: Seed, run, extract\n",
    "promptforge: 0\n",
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
    "name: papergate-missing-output\n",
    "description: Never writes its promised report\n",
    "promptforge: 0\n",
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
/// host's seeding and extraction half of the seed-run-extract round trip.
fn fresh_store_view(vfs: &VfsRef, origin: &str) -> Access {
    let access = vfs
        .acquire(Origin::new(origin))
        .expect("the prepared backend acquires");
    promptforge_vfs::detail::store_view(&access).expect("the handle declares a store")
}

/// The production host's extraction rule (pattern: papergate's app.rs): a
/// declared output the run did not leave behind is an explicit contract
/// error naming the prompt's promise, never a bare not-found.
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

/// Seeds the prompt's declared input through the run's prepared handle.
/// The seeding access drops here - its scope ends - so the run's own
/// scope never meets the host's.
fn seed_declared_input(vfs: &VfsRef, prompt: &Prompt, contents: &str) {
    let input = prompt
        .frontmatter()
        .input()
        .expect("the fixture declares an input");
    fresh_store_view(vfs, "seed_declared_input")
        .write(input.path(), contents.as_bytes())
        .expect("the declared input seeds");
}

/// Prepares a fixture run and returns the run's handle (for seeding
/// before and extraction after) plus the pending run: the host's
/// seed-run-extract sequence is prepare, seed through the handle, drive,
/// extract through the handle.
fn offline_run(
    prompt: &Prompt,
    execution: &'static str,
) -> (
    VfsRef,
    impl std::future::Future<Output = Result<String, crate::RunError>>,
) {
    let recorder = Arc::new(Recorder::default());
    let prompt = prompt.clone();
    let (ctx, host, vfs) = prepare_run(
        &prompt,
        &[],
        RunOptions {
            execution,
            observer: recorder,
        },
    );
    let run = async move { drive(&prompt, "", ctx, host).await };
    (vfs, run)
}

#[tokio::test]
async fn an_end_to_end_run_threads_one_vfs_ref_through_every_section() {
    // The store survives the context-clearing section transition: one
    // section's write is the next section's read, over the run's
    // prepared handle.
    let source = "\
---\nname: vfs-end-to-end\ndescription: d\npromptforge: 0\n---\n\n\
# Title\n\n\
## First\n\n\
```lua\n\
store.write('handoff.txt', 'across the reset')\n\
```\n\n\
## Second\n\n\
```lua\n\
return store.read('handoff.txt')\n\
```\n";
    let recorder = Arc::new(Recorder::default());
    let prompt = parse_execution_fixture(source, "vfs-end-to-end", "vfs-e2e", recorder.as_ref());
    let (ctx, host, vfs) = prepare_run(
        &prompt,
        &[],
        RunOptions {
            execution: "vfs-e2e",
            observer: recorder,
        },
    );
    let result = drive(&prompt, "", ctx, host)
        .await
        .expect("the run threads the prepared handle through both sections");
    assert_eq!(result, "across the reset");
    // Extraction after the run takes a fresh access: the run's identities
    // dropped with it, so nothing the run touched can conflict here.
    assert_eq!(
        fresh_store_view(&vfs, "prepared handle extraction")
            .read_string("handoff.txt")
            .expect("the run's write persists on the handle"),
        "across the reset"
    );
}

#[tokio::test]
async fn a_host_seeds_and_extracts_through_the_prepared_handle_with_no_real_files() {
    let recorder = Arc::new(Recorder::default());
    let prompt = parse_execution_fixture(
        ROUND_TRIP,
        "papergate-round-trip",
        "vfs-round-trip",
        recorder.as_ref(),
    );
    let (vfs, run) = offline_run(&prompt, "vfs-round-trip");
    seed_declared_input(&vfs, &prompt, "the paper body");
    let result = run.await.expect("the seeded run executes offline");
    assert_eq!(result, "done");
    let store = fresh_store_view(&vfs, "round-trip extraction");
    let report =
        extract_declared_output(&store, &prompt).expect("the run left its promised output");
    assert_eq!(report, "report on: the paper body");
}

#[tokio::test]
async fn a_missing_declared_output_is_a_contract_error_naming_the_prompts_promise() {
    let recorder = Arc::new(Recorder::default());
    let prompt = parse_execution_fixture(
        MISSING_OUTPUT,
        "papergate-missing-output",
        "vfs-missing-output",
        recorder.as_ref(),
    );
    let (vfs, run) = offline_run(&prompt, "vfs-missing-output");
    seed_declared_input(&vfs, &prompt, "the paper body");
    // The executor does not enforce the declaration; the run succeeds and
    // the host's extraction is where the broken promise surfaces.
    let result = run.await.expect("the run itself succeeds");
    assert_eq!(result, "read: the paper body");
    let store = fresh_store_view(&vfs, "missing-output extraction");
    let error = extract_declared_output(&store, &prompt)
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

/// A unique temporary directory that removes itself on drop. The suite has
/// no tempfile dependency; this mirrors promptforge-vfs's own test helper.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "promptforge-engine-vfs-{}-{unique}-{name}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&dir).expect("the temp dir creates");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_interleaving_is_invariant_across_memory_and_host_backends() {
    // The consistency rule: every store operation takes the leaf-yield path
    // uniformly, with no inline fast path, so a run's observable behavior
    // cannot depend on which backend serves the store mount. The same
    // fanout fixture (arm-scoped writes, a post-join glob, the ordered
    // merge) runs over the stock memory mount and over a host backend
    // rooted in a temp dir; the result and the stored contents must be
    // identical.
    const FANOUT_STORE_WRITES: &str =
        include_str!("../../../../tests/prompts/execution/fanout-store-writes.md");
    // Both arms drive the raw host-handle contract (no prepare pass), so
    // the caller's own backend serves the declared store in each.
    let memory = run_fixture(
        FANOUT_STORE_WRITES,
        "execution/fanout-store-writes.md",
        "vfs-invariance-memory",
        "",
        Some(VfsRef::default()),
    )
    .await;
    let memory_result = memory
        .result
        .expect("the memory-backed fanout must execute offline");

    let temp = TempDir::new("host-backend");
    let host_vfs = VfsRef::builder()
        .store(
            "/",
            HostBackend::rooted(&temp.0).expect("the temp dir roots the host backend"),
        )
        .build();
    let host = run_fixture(
        FANOUT_STORE_WRITES,
        "execution/fanout-store-writes.md",
        "vfs-invariance-host",
        "",
        Some(host_vfs),
    )
    .await;
    let host_result = host
        .result
        .expect("the host-backed fanout must execute offline");

    assert_eq!(
        memory_result, host_result,
        "the run's result must not depend on the backend"
    );
    for path in ["arm-1.md", "arm-2.md", "merged.md"] {
        assert_eq!(
            memory.store.read(path).ok(),
            host.store.read(path).ok(),
            "stored contents at {path} must not depend on the backend"
        );
    }
    // The host backend really served the mount: the arm's write landed on
    // the host filesystem under the root.
    assert!(
        temp.0.join("arm-1.md").is_file(),
        "the host backend must persist the arm's write under its root"
    );
}

const HELD_STORE: &str = "---\nname: held-store\ndescription: d\npromptforge: 0\n---\n\n\
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
fn a_run_ends_its_scope_at_done_while_the_host_still_holds_its_store_views() -> Result<(), VfsError>
{
    let vfs = VfsRef::default();
    let (run, result, held) = drive_holding_effects(&vfs);
    assert!(
        matches!(&result, RunResult::Ok(text) if text == "from the run"),
        "the run succeeds: {result:?}"
    );
    assert!(
        held.iter()
            .any(|effect| matches!(effect, Effect::Store { .. })),
        "the host holds the run's store views past Done: {held:?}"
    );
    // A fresh scope reads the run's write without a conflict: the run's
    // scope ended at Done, not when the run or the host's views dropped.
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
    let Some(Effect::Store { access, .. }) = held.first() else {
        panic!("the run's first effect is its store write: {held:?}");
    };
    match access.write("kept.txt", b"after the run") {
        Err(VfsError::PermissionDenied { path, reason }) => {
            assert_eq!(path, "kept.txt", "the refusal names the logical path");
            assert!(reason.contains("has ended"), "{reason}");
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
fn dropping_a_run_before_done_ends_its_scope_while_the_host_still_holds_its_store_views()
-> Result<(), VfsError> {
    let vfs = VfsRef::default();
    let mut run = held_store_run(&vfs);
    let mut held = Vec::new();
    let Step::Pending { effects, .. } = run.step() else {
        panic!("the run parks on its store write");
    };
    answer_holding(&mut run, effects, &mut held);
    let Step::Pending { effects, .. } = run.step() else {
        panic!("the run parks on its store read");
    };
    held.extend(effects.into_iter().map(|(_, _, effect)| effect));
    drop(run);
    // A fresh scope reads the dropped run's write without a conflict,
    // though the host still holds every store view the run handed out.
    let fresh = vfs.acquire(Origin::new("after drop"))?;
    assert_eq!(fresh.read("/kept.txt")?, b"from the run");
    drop(held);
    Ok(())
}
