//! The run surface a Host builds through the public API: a run over a
//! prompt string reports what it left at its declared output file, and
//! the output error a report carries.

use std::sync::Arc;

use harness::capability::{CapabilityRegistry, HostServices};
use harness::record::{MemoryRecorder, RunOutcome};
use harness::vfs::{Origin, VfsError, VfsRef};
use harness::{Harness, HostSnapshot, OutputError, RunRequest};

use crate::support::{Offline, TokioTimer};

/// A prompt that reads its declared input and writes its declared output.
const SHOUTS: &str = concat!(
    "---\nname: shouts\ndescription: Shouts its input\npromptforge: 0\n",
    "input: { path: line.txt, description: The line to shout }\n",
    "output: { path: shout.txt, description: The shouted line }\n",
    "---\n\n# Shouts\n\n## Only\n\n```lua\n",
    "store.write('shout.txt', string.upper(store.read('line.txt')))\n",
    "```\n",
);

fn harness() -> Harness {
    Harness::new(
        Arc::new(MemoryRecorder::new()),
        Arc::new(Offline),
        Arc::new(TokioTimer),
        CapabilityRegistry::new(),
        HostServices::new(),
    )
}

fn request(source: &str, vfs: VfsRef) -> RunRequest {
    RunRequest {
        name: "shouts-1".to_owned(),
        source: source.to_owned(),
        args: String::new(),
        input_text: Some("hello".to_owned()),
        vfs,
        host: HostSnapshot::default(),
    }
}

#[tokio::test]
async fn a_run_over_a_prompt_string_reports_its_output() {
    let vfs = VfsRef::default();
    let report = harness()
        .run(request(SHOUTS, vfs.clone()))
        .await
        .expect("the run reaches an outcome");
    assert!(matches!(report.outcome, RunOutcome::Completed { .. }));
    assert_eq!(
        report.output,
        Ok("HELLO".to_owned()),
        "the report carries what the run left at its declared output file"
    );
    assert_eq!(
        vfs.acquire_store(Origin::new("suite check"))
            .expect("the default handle declares a store")
            .read_string("shout.txt")
            .expect("the output stays in the Host's store"),
        "HELLO"
    );
}

#[tokio::test]
async fn a_run_that_does_not_complete_reports_no_output() {
    let failing = SHOUTS.replace("store.write", "error('no shout') store.write");
    let report = harness()
        .run(request(&failing, VfsRef::default()))
        .await
        .expect("the run reaches an outcome");
    assert!(matches!(report.outcome, RunOutcome::Failed { .. }));
    assert_eq!(report.output, Err(OutputError::NotCompleted));
}

#[test]
fn an_output_store_failure_renders_its_own_message_and_sources_the_engines_error() {
    let error = OutputError::Vfs {
        path: "report.md".to_owned(),
        source: VfsError::NotFound {
            path: "report.md".to_owned(),
        },
    };
    assert_eq!(
        error.to_string(),
        "the output file `report.md` could not be read"
    );
    assert!(std::error::Error::source(&error).is_some());
}
