//! The prompt's declared store files: the launch's input text is staged
//! at the frontmatter's `input:` path before the run, through the store
//! view wherever the handle mounts the store; a Host-seeded store
//! satisfies the declaration; text with no declared file, a declared file
//! with neither text nor a seeded copy, and a store that refuses the write
//! each refuse the run and end it under the `Input` kind; and the
//! declared `output:` path comes back on the prepared run for the caller
//! to read once the run completes.

use std::sync::Arc;

use harness_runner::display_chain;
use harness_runner::effect_loop::drive_run;
use harness_runner::files::{InputFileError, read_output};
use harness_runner::prepare::{PrepareError, Prepared, prepare_run};
use harness_runner::recorder::{MemoryRecorder, RunOutcome};
use promptforge::cancel::CancelHandle;
use promptforge::vfs::{MemoryBackend, Mode, ModePolicy, Origin, VfsError, VfsRef};

use super::{
    PLAIN, assert_events_are_the_recorded_ones, completed, prompt_file, recorder, services,
};

/// A prompt declaring `paper.md` as its input and `report.md` as its
/// output, whose one section writes the output from the input.
const FILES: &str = "---\nname: files\ndescription: d\npromptforge: 0\n\
    input:\n  path: paper.md\n  description: The paper\n\
    output:\n  path: report.md\n  description: The report\n---\n\n\
    # Title\n\n## Only\n\n```lua\n\
    store.write('report.md', 'seen: ' .. store.read('paper.md'))\nreturn 'done'\n```\n";

/// Prepares `source` over `vfs` with `input_text` and returns the result.
async fn prepare(
    source: &str,
    vfs: &VfsRef,
    input_text: Option<&str>,
) -> (Arc<MemoryRecorder>, Result<Prepared, PrepareError>) {
    let dir = tempfile::tempdir().unwrap();
    let recorder = recorder();
    let mut services = services(&recorder, None);
    services.vfs = vfs.clone();
    services.input_text = input_text.map(str::to_owned);
    let prepared = prepare_run(&prompt_file(dir.path(), source), "", services).await;
    (recorder, prepared)
}

/// Checks that `error` is an input refusal whose run ended under the
/// `Input` kind with the cause chain as its message, and that it carries
/// the parse events the recorder holds. Returns the cause.
fn refused(recorder: &MemoryRecorder, error: PrepareError) -> InputFileError {
    let PrepareError::Input {
        run_id,
        events,
        source,
    } = error
    else {
        panic!("the failure is an input refusal: {error}");
    };
    assert_eq!(
        recorder.outcome(run_id),
        Some(RunOutcome::Failed {
            kind: "Input".to_owned(),
            message: display_chain(&source),
        }),
        "the recorder holds the refusal under the Input kind"
    );
    assert_events_are_the_recorded_ones(recorder, run_id, &events);
    source
}

#[tokio::test]
async fn the_input_text_is_staged_at_the_declared_path_and_the_output_path_comes_back() {
    let vfs = VfsRef::default();
    let (recorder, prepared) = prepare(FILES, &vfs, Some("# Paper")).await;
    let prepared = prepared.expect("a supplied declared input prepares");
    assert_eq!(prepared.output_path.as_deref(), Some("report.md"));
    let outcome = drive_run(
        prepared.run,
        prepared.performers,
        recorder.clone(),
        prepared.run_id,
        CancelHandle::new(),
        |_event| {},
    )
    .await
    .unwrap();
    assert_eq!(completed(outcome), "done");
    assert_eq!(read_output(&vfs, "report.md").unwrap(), "seen: # Paper");
}

#[tokio::test]
async fn a_store_mounted_away_from_the_root_is_staged_by_its_logical_path() {
    let vfs = VfsRef::builder()
        .mount("/", MemoryBackend::new())
        .store("/store", MemoryBackend::new())
        .build();
    let (_recorder, prepared) = prepare(FILES, &vfs, Some("# Paper")).await;
    prepared.expect("a supplied declared input prepares");
    let access = vfs.acquire(Origin::new("prepare files test")).unwrap();
    assert_eq!(access.read("/store/paper.md").unwrap(), b"# Paper");
    assert!(
        !access.exists("/paper.md").unwrap(),
        "the base never saw it"
    );
}

#[tokio::test]
async fn a_host_seeded_store_satisfies_the_declared_input() {
    let vfs = VfsRef::default();
    vfs.acquire_store(Origin::new("prepare files test"))
        .unwrap()
        .write("paper.md", b"# Seeded")
        .unwrap();
    let (_recorder, prepared) = prepare(FILES, &vfs, None).await;
    prepared.expect("an input the store already holds prepares");
}

#[tokio::test]
async fn input_text_for_a_prompt_that_declares_no_input_is_refused() {
    let (recorder, prepared) = prepare(PLAIN, &VfsRef::default(), Some("# Paper")).await;
    let source = refused(&recorder, prepared.expect_err("undeclared input refuses"));
    assert!(matches!(source, InputFileError::Undeclared), "{source:?}");
}

#[tokio::test]
async fn a_declared_input_neither_supplied_nor_in_the_store_is_refused() {
    let (recorder, prepared) = prepare(FILES, &VfsRef::default(), None).await;
    let source = refused(&recorder, prepared.expect_err("a missing input refuses"));
    let InputFileError::Missing { path } = source else {
        panic!("the refusal names the missing file: {source:?}");
    };
    assert_eq!(path, "paper.md");
}

#[tokio::test]
async fn a_store_that_refuses_the_staging_write_refuses_the_run() {
    let vfs = VfsRef::builder()
        .store("/", MemoryBackend::new())
        .policy(ModePolicy::new(Mode::Ask))
        .build();
    let (recorder, prepared) = prepare(FILES, &vfs, Some("# Paper")).await;
    let source = refused(&recorder, prepared.expect_err("a refused write refuses"));
    let InputFileError::Vfs { path, source } = source else {
        panic!("the refusal is the store's: {source:?}");
    };
    assert_eq!(path, "paper.md");
    assert!(
        matches!(source, VfsError::PermissionDenied { .. }),
        "{source:?}"
    );
}

#[tokio::test]
async fn a_prompt_without_an_output_declaration_has_no_output_path() {
    let (_recorder, prepared) = prepare(PLAIN, &VfsRef::default(), None).await;
    assert_eq!(prepared.unwrap().output_path, None);
}
