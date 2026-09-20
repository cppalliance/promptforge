//! Activation against the registry: missing required reported, absent
//! optional skipped and logged, the run's services reaching `create`,
//! activation failure semantics, and the run path's refusals.

use harness_capabilities::{CapabilityId, CapabilityRegistry};
use promptforge_api_runtime::execute::{Environment, RunErrorKind, RunResult};
use promptforge_api_types::cancel::CancelHandle;
use shared_vfs::Origin;

use super::support::{
    Fixture, STORE_MOUNT, captured_logs, context, parse, prepare_activated, run_activated,
};

/// A prompt declaring `promptforge/web` as a required capability.
pub(super) const DECLARES_REQUIRED: &str = concat!(
    "---\n",
    "name: declares-required\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - promptforge/web\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring `promptforge/web` as an optional capability.
const DECLARES_OPTIONAL: &str = concat!(
    "---\n",
    "name: declares-optional\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - ref: promptforge/web\n",
    "    optional: true\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring `promptforge/web` as required and returning the
/// marker its activation wrote into the run's store.
const READS_ACTIVATION_MARKER: &str = concat!(
    "---\n",
    "name: reads-activation-marker\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - promptforge/web\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "```lua\n",
    "return store.read('activated.txt')\n",
    "```\n",
);

#[test]
fn a_missing_required_capability_is_reported() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    // No registry: activation reports the declared required capability
    // absent, and the merged prepare report carries it.
    let (_ctx, requirements, _) = prepare_activated(
        Environment::new(),
        None,
        &prompt,
        context("prepare-missing"),
    );
    assert!(requirements.unmet_requirements.is_empty());
    assert_eq!(
        requirements.missing_required,
        [CapabilityId::parse("promptforge/web").expect("the id is valid")]
    );
    assert!(!requirements.is_satisfied());
}

#[test]
fn an_absent_optional_capability_is_skipped_and_logged() {
    let prompt = parse(DECLARES_OPTIONAL, "declares-optional");
    let logs = captured_logs(|| {
        let (_ctx, requirements, _) = prepare_activated(
            Environment::new(),
            None,
            &prompt,
            context("prepare-optional"),
        );
        assert!(requirements.missing_required.is_empty());
        assert!(requirements.is_satisfied());
    });
    assert!(
        logs.contains("promptforge/web"),
        "the skip log line names the capability: {logs}"
    );
}

#[test]
fn activation_receives_the_runs_own_services() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let (fixture, activations) = Fixture::new("promptforge/web", false);
    let mut registry = CapabilityRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    let cancel = CancelHandle::new();
    let (ctx, requirements, _) = prepare_activated(
        Environment::new(),
        Some(&registry),
        &prompt,
        context("prepare-services").cancel(cancel.clone()),
    );
    assert!(requirements.is_satisfied());
    // The host-supplied cancellation handle reached `create` unchanged.
    let activations = activations.lock().expect("the lock is not poisoned");
    assert_eq!(activations.len(), 1, "create ran exactly once");
    assert_eq!(activations[0].marker.as_deref(), Some("active"));
    assert!(!activations[0].cancel.is_cancelled());
    cancel.cancel();
    assert!(
        activations[0].cancel.is_cancelled(),
        "the activated handle is the run's own"
    );
    drop(activations);
    // The services VFS is the run's own handle: the host built the run's
    // router, handed it to activation, and set it on the context, so the
    // activation's marker is readable through the context's store mount.
    let access = ctx
        .vfs_handle()
        .acquire(Origin::new("post-prepare read"))
        .expect("the prepared handle acquires");
    let marker = format!("{STORE_MOUNT}/activated.txt");
    assert_eq!(
        access.read(&marker).expect("the marker persists"),
        b"active"
    );
}

#[test]
fn a_required_activation_failure_is_logged_and_reported() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let (fixture, _activations) = Fixture::new("promptforge/web", true);
    let mut registry = CapabilityRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    let logs = captured_logs(|| {
        // A present-but-failing required capability leaves the run
        // without something the prompt declared: it is reported like an
        // absent one, and the failure is also a log line.
        let (_ctx, requirements, _) = prepare_activated(
            Environment::new(),
            Some(&registry),
            &prompt,
            context("prepare-failing"),
        );
        assert_eq!(
            requirements.missing_required,
            [CapabilityId::parse("promptforge/web").expect("the id is valid")]
        );
        assert!(!requirements.is_satisfied());
    });
    assert!(
        logs.contains("promptforge/web"),
        "the failure log line names the capability: {logs}"
    );
}

#[test]
fn an_optional_activation_failure_is_logged_and_contributes_nothing() {
    let prompt = parse(DECLARES_OPTIONAL, "declares-optional");
    let (fixture, _activations) = Fixture::new("promptforge/web", true);
    let mut registry = CapabilityRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    let logs = captured_logs(|| {
        // An optional capability that fails to activate is only a log
        // line: the prompt declared it could run without.
        let (_ctx, requirements, _) = prepare_activated(
            Environment::new(),
            Some(&registry),
            &prompt,
            context("prepare-failing"),
        );
        assert!(requirements.is_satisfied());
    });
    assert!(
        logs.contains("promptforge/web"),
        "the failure log line names the capability: {logs}"
    );
}

#[tokio::test]
async fn the_run_path_refuses_a_missing_required_capability_with_a_notice_naming_it() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    // An empty registry: activation reports the declared required
    // capability absent, and the run path folds that report into its
    // refusal.
    let result = run_activated(
        CapabilityRegistry::new(),
        &prompt,
        context("refuse-missing"),
    )
    .await;
    let RunResult::Failure(error) = result else {
        panic!("a prompt missing a required capability is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = error.to_string();
    assert!(
        notice.contains("missing required capability: promptforge/web"),
        "the notice names the missing capability: {notice}"
    );
}

#[tokio::test]
async fn the_run_path_activates_over_the_store_the_run_reads() {
    let prompt = parse(READS_ACTIVATION_MARKER, "reads-activation-marker");
    let (fixture, activations) = Fixture::new("promptforge/web", false);
    let mut registry = CapabilityRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    // The run path activates exactly once, over the run's own router: the
    // marker the capability wrote through its services is what the prompt
    // reads back through `store`.
    let result = run_activated(registry, &prompt, context("activate-once")).await;
    let RunResult::Ok(text) = result else {
        panic!("the activated run reads its capability's marker: {result:?}");
    };
    assert_eq!(text, "active");
    assert_eq!(
        activations.lock().expect("the lock is not poisoned").len(),
        1,
        "create ran exactly once"
    );
}
