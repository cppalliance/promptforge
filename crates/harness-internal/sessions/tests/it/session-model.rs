//! The model a launch binds, resolved through the broker's model list: the
//! Host's selection when one is set, the list's first model when none is,
//! and a run that fails as `RunFailed` when the selection is absent from
//! the list or the list cannot be fetched.

use harness_sessions::environment::HostSnapshot;
use harness_sessions::session::{FailureKind, SessionFailure};
use promptforge::model::CompletionErrorKind;

use super::*;
use crate::support::{Rounds, ScriptedBroker};

/// A program whose one model call is a tool-less infer through its one
/// declared role, so the round names the model the launch bound.
const RESOLVES: &str = "---\nname: resolves\ndescription: infers once\npromptforge: 0\n\
    models:\n  writer: {}\n---\n\n\
    # Resolves\n\n```lua\nmodels.default('writer')\n```\n\n\
    ## Only\n\n```lua\nreturn models.infer('prose')\n```\n";

/// A Harness on `broker` over a fresh agents directory holding
/// `resolves.md`, with a usable catalog bound at generation 1 and the
/// Host's selection set to `selected`.
fn harness_on(dir: &Path, broker: ScriptedBroker, selected: Option<&str>) -> Harness {
    let agents = dir.join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("resolves.md"), RESOLVES).unwrap();
    let harness = Harness::new(
        HarnessConfig {
            agents_path: agents,
        },
        Arc::new(MemoryRecorder::new()),
        Arc::new(broker),
        CapabilityRegistry::new(),
        HostServices::new(),
    );
    harness.set_catalog(CatalogBinding {
        generation: 1,
        models: vec![idless_chat_model()],
    });
    harness.set_host(HostSnapshot {
        selected_model: selected.map(str::to_owned),
        ..HostSnapshot::default()
    });
    harness
}

/// Runs `resolves` on `broker` under `selected` to its end and returns
/// the model each round named.
async fn bound_models(broker: ScriptedBroker, selected: Option<&str>) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let rounds: Rounds = broker.rounds();
    let harness = harness_on(dir.path(), broker, selected);
    let session = launch_agent(&harness, "resolves").await;
    wait_for(&session, SessionState::Closed).await;
    rounds
        .lock()
        .unwrap()
        .iter()
        .map(|round| round.model.clone())
        .collect()
}

/// Launches `resolves` on `broker` under `selected` and returns the one
/// failure report its session made before closing.
async fn failed_launch(broker: ScriptedBroker, selected: Option<&str>) -> SessionFailure {
    let dir = tempfile::tempdir().unwrap();
    let rounds: Rounds = broker.rounds();
    let harness = harness_on(dir.path(), broker, selected);
    let session = launch_agent(&harness, "resolves").await;
    // The supervisor has not run yet on this single-threaded runtime, so
    // subscribing here sees the run's failure.
    let mut errors = session.subscribe_errors();
    let failure = tokio::time::timeout(PATIENCE, errors.recv())
        .await
        .expect("the run reports its failure in time")
        .expect("the failure report arrives");
    wait_for(&session, SessionState::Closed).await;
    assert!(
        rounds.lock().unwrap().is_empty(),
        "a run without a model makes no round"
    );
    assert!(
        session.run_ids().is_empty(),
        "the run fails before the recorder begins it"
    );
    failure
}

#[tokio::test]
async fn a_launch_binds_the_selected_model_from_the_brokers_list() {
    let broker = ScriptedBroker::new(&["first-model", "chosen-model"], "done", "served");
    assert_eq!(
        bound_models(broker, Some("chosen-model")).await,
        ["chosen-model"],
        "the round names the Host's selection, looked up in the broker's list"
    );
}

#[tokio::test]
async fn with_no_selection_a_launch_binds_the_first_model_of_the_brokers_list() {
    let broker = ScriptedBroker::new(&["first-model", "second-model"], "done", "served");
    assert_eq!(
        bound_models(broker, None).await,
        ["first-model"],
        "the round names the first model the broker lists"
    );
}

#[tokio::test]
async fn a_selection_absent_from_the_brokers_list_fails_the_run() {
    let broker = ScriptedBroker::new(&["first-model"], "done", "served");
    let failure = failed_launch(broker, Some("gone-model")).await;
    assert_eq!(failure.kind, FailureKind::RunFailed);
    assert!(
        failure.message.contains("gone-model"),
        "the report names the absent selection: {}",
        failure.message
    );
}

#[tokio::test]
async fn a_failed_model_listing_fails_the_run_with_the_brokers_message() {
    let kind = CompletionErrorKind::Transport;
    let failure = failed_launch(ScriptedBroker::failing_models(kind), Some("any-model")).await;
    assert_eq!(failure.kind, FailureKind::RunFailed);
    assert!(
        failure.message.contains(kind.phrase()),
        "the report carries the broker's message: {}",
        failure.message
    );
}
