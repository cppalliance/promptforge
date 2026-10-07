//! Activation against the registry: a missing declared Plugin reported,
//! the run's services reaching `create`, activation failure semantics,
//! and the run path's refusals.

use harness_plugins::{PluginId, PluginRegistry};
use promptforge::cancel::CancelHandle;
use promptforge::{Environment, RunErrorKind, RunResult};

use super::support::{Fixture, captured_logs, context, parse, prepare_activated, run_activated};

/// A prompt declaring `web` as a required Plugin.
pub(super) const DECLARES_REQUIRED: &str = concat!(
    "---\n",
    "name: declares-required\n",
    "description: d\n",
    "promptforge: 0\n",
    "plugins:\n",
    "  - web\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring `web` as required and returning a fixed
/// text from Lua, so the run completes without a model.
const RUNS_AFTER_ACTIVATION: &str = concat!(
    "---\n",
    "name: runs-after-activation\n",
    "description: d\n",
    "promptforge: 0\n",
    "plugins:\n",
    "  - web\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "```lua\n",
    "return 'ran'\n",
    "```\n",
);

#[test]
fn a_missing_required_plugin_is_reported() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    // No registry: activation reports the declared required Plugin
    // absent, and the merged prepare report includes it.
    let (_ctx, requirements, _) = prepare_activated(
        Environment::new(),
        None,
        &prompt,
        context("prepare-missing"),
    );
    assert!(requirements.unmet_requirements.is_empty());
    assert_eq!(
        requirements.missing_required,
        [PluginId::parse("web").expect("the id is valid")]
    );
    assert!(!requirements.is_satisfied());
}

#[test]
fn activation_receives_the_runs_own_services() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let (fixture, activations) = Fixture::new("web", false);
    let mut registry = PluginRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    let cancel = CancelHandle::new();
    let (_ctx, requirements, _) = prepare_activated(
        Environment::new(),
        Some(&registry),
        &prompt,
        context("prepare-services").cancel(cancel.clone()),
    );
    assert!(requirements.is_satisfied());
    // The Harness-supplied cancellation handle reached `create` unchanged.
    let activations = activations.lock().expect("the lock is not poisoned");
    assert_eq!(activations.len(), 1, "create ran exactly once");
    assert!(!activations[0].cancel.is_cancelled());
    cancel.cancel();
    assert!(
        activations[0].cancel.is_cancelled(),
        "the activated handle is the run's own"
    );
}

#[test]
fn a_required_activation_failure_is_logged_and_reported() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let (fixture, _activations) = Fixture::new("web", true);
    let mut registry = PluginRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    let logs = captured_logs(|| {
        // A present-but-failing required Plugin leaves the run
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
            [PluginId::parse("web").expect("the id is valid")]
        );
        assert!(!requirements.is_satisfied());
    });
    assert!(
        logs.contains("web"),
        "the failure log line names the Plugin: {logs}"
    );
}

#[test]
fn the_run_path_refuses_a_missing_required_plugin_with_a_notice_naming_it() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    // An empty registry: activation reports the declared required
    // Plugin absent, and the run path folds that report into its
    // refusal.
    let result = run_activated(&PluginRegistry::new(), &prompt, context("refuse-missing"));
    let RunResult::Failure(error) = result else {
        panic!("a prompt missing a required Plugin is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = error.to_string();
    assert!(
        notice.contains("missing required Plugin: web"),
        "the notice names the missing Plugin: {notice}"
    );
}

#[test]
fn the_run_path_refuses_a_declared_plugin_whose_activation_fails() {
    let prompt = parse(RUNS_AFTER_ACTIVATION, "runs-after-activation");
    let (fixture, _activations) = Fixture::new("web", true);
    let mut registry = PluginRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    let result = run_activated(&registry, &prompt, context("refuse-failing"));
    let RunResult::Failure(error) = result else {
        panic!("a declared Plugin that activates to nothing refuses the run: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = error.to_string();
    assert!(
        notice.contains("missing required Plugin: web"),
        "the notice names the Plugin: {notice}"
    );
}

#[test]
fn the_run_path_activates_a_declared_plugin_exactly_once() {
    let prompt = parse(RUNS_AFTER_ACTIVATION, "runs-after-activation");
    let (fixture, activations) = Fixture::new("web", false);
    let mut registry = PluginRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    let result = run_activated(&registry, &prompt, context("activate-once"));
    let RunResult::Ok(text) = result else {
        panic!("the activated run completes: {result:?}");
    };
    assert_eq!(text, "ran");
    assert_eq!(
        activations.lock().expect("the lock is not poisoned").len(),
        1,
        "create ran exactly once"
    );
}
