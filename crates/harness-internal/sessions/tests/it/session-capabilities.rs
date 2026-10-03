//! The capabilities a session's runs resolve against: the Host's
//! registry alone, which supplies `promptforge/user-input` and
//! `promptforge/web` only when the Host registered them.

use std::sync::Arc;

use harness_capabilities::CapabilityRegistry;
use harness_runner::recorder::{MemoryRecorder, RunOutcome};
use harness_sessions::environment::CatalogBinding;
use harness_sessions::transition::SessionState;

use super::{idless_chat_model, launch, launch_agent, unbound_harness_over, wait_for};

#[tokio::test]
async fn a_harness_whose_registry_lacks_user_input_refuses_an_agent_requiring_it() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(MemoryRecorder::new());
    let harness = unbound_harness_over(dir.path(), recorder.clone(), CapabilityRegistry::new());
    harness.set_catalog(CatalogBinding {
        generation: 1,
        models: vec![idless_chat_model()],
    });
    let session = launch(&harness).await;
    wait_for(&session, SessionState::Closed).await;
    let runs = session.run_ids();
    assert_eq!(runs.len(), 1, "the refused preparation began one run");
    assert_eq!(
        recorder.outcome(runs[0]),
        Some(RunOutcome::Failed {
            kind: "RequirementsUnmet".to_owned(),
            message: "the environment cannot satisfy this prompt:\n\
                - missing required capability: promptforge/user-input"
                .to_owned(),
        }),
        "the run resolves against the Host's registry, which lacks user input"
    );
}

/// A prompt that requires the web capability and returns a fixed text.
const BROWSES: &str = "---\nname: browses\ndescription: needs web\npromptforge: 0\n\
    capabilities:\n  - promptforge/web\n---\n\n\
    # Browses\n\n## Only\n\n```lua\nreturn 'browsed'\n```\n";

#[tokio::test]
async fn a_harness_whose_registry_lacks_web_refuses_an_agent_requiring_it() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(MemoryRecorder::new());
    let harness = unbound_harness_over(dir.path(), recorder.clone(), CapabilityRegistry::new());
    std::fs::write(dir.path().join("agents").join("browses.md"), BROWSES).unwrap();
    harness.set_catalog(CatalogBinding {
        generation: 1,
        models: vec![idless_chat_model()],
    });
    let session = launch_agent(&harness, "browses").await;
    wait_for(&session, SessionState::Closed).await;
    let runs = session.run_ids();
    assert_eq!(runs.len(), 1, "the refused preparation began one run");
    assert_eq!(
        recorder.outcome(runs[0]),
        Some(RunOutcome::Failed {
            kind: "RequirementsUnmet".to_owned(),
            message: "the environment cannot satisfy this prompt:\n\
                - missing required capability: promptforge/web"
                .to_owned(),
        }),
        "the Harness adds no web of its own to the Host's registry"
    );
}
