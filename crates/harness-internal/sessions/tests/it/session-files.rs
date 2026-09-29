//! A prompt's declared files through a session: the launch's input text is
//! staged at the declared input path and the completed run's declared
//! output comes back from `Session::output_text`; a host filesystem
//! handed over through `launch_with` is the one every run works in, its
//! store mounted wherever the host put it and its other mounts untouched;
//! a host-seeded store satisfies the declared input; a prompt without an
//! output file, and a run that never writes its own, report why; and a
//! declared input with nothing to stage fails the run as `RunFailed`.

use std::path::Path;

use harness_sessions::protocol::LaunchRequest;
use harness_sessions::runtime::{Harness, LaunchOptions};
use harness_sessions::session::{FailureKind, OutputError, Session};
use harness_sessions::transition::SessionState;
use promptforge::vfs::{MemoryBackend, Origin, VfsRef};

use super::{PATIENCE, harness, wait_for};

/// A prompt declaring `paper.md` in and `report.md` out, whose one
/// section writes the report from the paper.
const COPIES: &str = "---\nname: copies\ndescription: copies its input to its output\n\
    promptforge: 0\ninput:\n  path: paper.md\n  description: The paper\n\
    output:\n  path: report.md\n  description: The report\n---\n\n\
    # Copies\n\n## Only\n\n```lua\n\
    store.write('report.md', 'seen: ' .. store.read('paper.md'))\nreturn 'done'\n```\n";

/// The same declarations, but the section never writes the report.
const FORGETS: &str = "---\nname: forgets\ndescription: never writes its output\n\
    promptforge: 0\ninput:\n  path: paper.md\n  description: The paper\n\
    output:\n  path: report.md\n  description: The report\n---\n\n\
    # Forgets\n\n## Only\n\n```lua\nlocal _ = store.read('paper.md')\nreturn 'done'\n```\n";

/// A prompt that declares no files.
const PLAIN: &str = "---\nname: plain\ndescription: declares no files\npromptforge: 0\n---\n\n\
    # Plain\n\n## Only\n\n```lua\nreturn 'plain'\n```\n";

/// A harness over `dir` whose agents directory also holds `name.md`.
fn harness_with(dir: &Path, name: &str, source: &str) -> Harness {
    let harness = harness(dir);
    std::fs::write(dir.join("agents").join(format!("{name}.md")), source).unwrap();
    harness
}

fn request(agent: &str, input_text: Option<&str>) -> LaunchRequest {
    LaunchRequest {
        agent: agent.to_owned(),
        args: String::new(),
        input_text: input_text.map(str::to_owned),
    }
}

/// Launches `request` under `options` and waits for the session to end.
async fn run_to_close(
    harness: &Harness,
    request: LaunchRequest,
    options: LaunchOptions,
) -> Session {
    let session = harness
        .launch_with(request, options)
        .await
        .expect("the discovered agent launches");
    wait_for(&session, SessionState::Closed).await;
    session
}

#[tokio::test]
async fn the_input_text_round_trips_through_the_declared_output() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_with(dir.path(), "copies", COPIES);
    let session = harness
        .launch(request("copies", Some("# Paper")))
        .await
        .expect("the discovered agent launches");
    assert_eq!(
        session.output_text(),
        Err(OutputError::Unfinished),
        "no run has completed yet"
    );
    wait_for(&session, SessionState::Closed).await;
    assert_eq!(session.output_text(), Ok("seen: # Paper".to_owned()));
}

#[tokio::test]
async fn a_host_filesystem_is_the_one_the_session_works_in() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_with(dir.path(), "copies", COPIES);
    let extra = MemoryBackend::new();
    VfsRef::new(extra.clone())
        .acquire(Origin::new("session files test"))
        .unwrap()
        .write("/notes.md", b"host notes")
        .unwrap();
    let vfs = VfsRef::builder()
        .mount("/", MemoryBackend::new())
        .store("/store", MemoryBackend::new())
        .build()
        .overlay("/extra", extra);
    let options = LaunchOptions {
        vfs: Some(vfs.clone()),
    };
    let session = run_to_close(&harness, request("copies", Some("# Paper")), options).await;
    assert_eq!(session.output_text(), Ok("seen: # Paper".to_owned()));

    // The run staged and wrote under the host's store root, and the host
    // mount beside it kept its own file.
    let access = vfs.acquire(Origin::new("session files test")).unwrap();
    assert_eq!(access.read("/store/paper.md").unwrap(), b"# Paper");
    assert_eq!(access.read("/store/report.md").unwrap(), b"seen: # Paper");
    assert_eq!(access.read("/extra/notes.md").unwrap(), b"host notes");
}

#[tokio::test]
async fn a_host_seeded_store_satisfies_the_declared_input() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_with(dir.path(), "copies", COPIES);
    let vfs = VfsRef::default();
    vfs.acquire_store(Origin::new("session files test"))
        .unwrap()
        .write("paper.md", b"# Seeded")
        .unwrap();
    let options = LaunchOptions { vfs: Some(vfs) };
    let session = run_to_close(&harness, request("copies", None), options).await;
    assert_eq!(session.output_text(), Ok("seen: # Seeded".to_owned()));
}

#[tokio::test]
async fn a_prompt_without_an_output_file_reports_it_undeclared() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_with(dir.path(), "plain", PLAIN);
    let session = run_to_close(&harness, request("plain", None), LaunchOptions::default()).await;
    assert_eq!(session.output_text(), Err(OutputError::Undeclared));
}

#[tokio::test]
async fn a_run_that_never_writes_its_output_reports_it_missing() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_with(dir.path(), "forgets", FORGETS);
    let session = run_to_close(
        &harness,
        request("forgets", Some("# Paper")),
        LaunchOptions::default(),
    )
    .await;
    assert_eq!(
        session.output_text(),
        Err(OutputError::Missing {
            path: "report.md".to_owned()
        })
    );
}

#[tokio::test]
async fn a_declared_input_with_nothing_to_stage_fails_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_with(dir.path(), "copies", COPIES);
    let session = harness
        .launch(request("copies", None))
        .await
        .expect("the discovered agent launches");
    let mut errors = session.subscribe_errors();
    wait_for(&session, SessionState::Closed).await;
    let failure = tokio::time::timeout(PATIENCE, errors.recv())
        .await
        .expect("the refusal is reported in time")
        .expect("the failure report arrives");
    assert_eq!(failure.kind, FailureKind::RunFailed);
    assert!(
        failure.message.contains("paper.md"),
        "the report names the missing file: {}",
        failure.message
    );
    assert_eq!(session.output_text(), Err(OutputError::Unfinished));
}
