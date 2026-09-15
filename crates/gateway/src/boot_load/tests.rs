//! Unit tests for the boot load: the phases that need no child process.

use gateway_config::{Config, ProfileName};
use tokio_util::sync::CancellationToken;

use crate::error::GatewayError;
use crate::test_support::boot_state;

const CATALOG: &str = "config-version = 0\n\
     [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
     [[endpoint]]\nid = \"fake\"\nprotocol = \"openai\"\nbase_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
     [[model]]\nname = \"alpha-model\"\ndescription = \"alpha\"\ncontext = 1024\nupstream = \"alpha\"\nendpoints = [\"fake\"]\n\
     [[local_model]]\nname = \"alpha-local\"\ndescription = \"local\"\nsource = \"/models/alpha.gguf\"\ncontext = 4096\n\
     [[profile]]\nname = \"alpha\"\nmodels = [\"alpha-local\"]\n\
     [[profile]]\nname = \"remote\"\nmodels = []\n";

fn state() -> crate::AppState {
    boot_state(Config::from_toml_str(CATALOG).expect("catalog parses"))
}

fn name(profile: &str) -> ProfileName {
    ProfileName::parse(profile).expect("profile name")
}

/// A profile with no local members loads at once: the remote table the
/// runner published is untouched and nothing is promised as loading.
#[tokio::test]
async fn a_remote_only_profile_loads_without_touching_the_routing_table() {
    let state = state();
    let tree = state.hub.operation();
    let token = CancellationToken::new();

    let outcome = super::load_local(&state, &name("remote"), &tree, &token).await;

    assert!(outcome.is_ok(), "nothing local to load: {outcome:?}");
    let live = state.live.read().await;
    assert!(live.routing.model("alpha-model").is_ok());
    assert!(live.routing.model("alpha-local").is_err());
    assert!(live.loading.is_empty());
}

/// A name the catalog does not define fails the `loading-profile` leaf
/// and changes nothing.
#[tokio::test]
async fn an_undefined_profile_is_profile_not_found() {
    let state = state();
    let tree = state.hub.operation();
    let token = CancellationToken::new();

    let outcome = super::load_local(&state, &name("ghost"), &tree, &token).await;

    assert!(
        matches!(&outcome, Err(GatewayError::ProfileNotFound(missing)) if missing == "ghost"),
        "the miss names the profile: {outcome:?}"
    );
    assert!(state.live.read().await.loading.is_empty());
}

/// A token fired before the load begins stops it before any leaf.
#[tokio::test]
async fn a_pre_cancelled_load_changes_nothing() {
    let state = state();
    let tree = state.hub.operation();
    let token = CancellationToken::new();
    token.cancel();

    let outcome = super::load_local(&state, &name("alpha"), &tree, &token).await;

    assert!(
        matches!(outcome, Err(GatewayError::CommandCancelled(_))),
        "a fired token is explicit: {outcome:?}"
    );
    assert!(state.live.read().await.loading.is_empty());
}

/// The commit routes the ready children beside the remote models,
/// installs the runtime, and withdraws the loading promise.
#[cfg(all(feature = "local", feature = "test-fixtures"))]
#[tokio::test]
async fn the_commit_routes_the_ready_children_and_clears_loading() {
    let state = state();
    state
        .live
        .write()
        .await
        .loading
        .insert("alpha-local".to_owned());
    let child = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         [[endpoint]]\nid = \"local\"\nprotocol = \"openai\"\nbase_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
         [[model]]\nname = \"alpha-local\"\ndescription = \"running child\"\ncontext = 4096\nupstream = \"alpha-local\"\nendpoints = [\"local\"]\n",
    )
    .expect("child config parses");
    let routing = crate::routing::Routing::from_config(&child).expect("child routes");
    let runtime = crate::local::LocalRuntime::from_test_models(routing.models().to_vec());

    super::commit(&state, &name("alpha"), runtime, Vec::new())
        .await
        .expect("a full start commits");

    let live = state.live.read().await;
    assert!(live.routing.model("alpha-model").is_ok(), "remote stays");
    assert!(live.routing.model("alpha-local").is_ok(), "local joins");
    assert_eq!(live.local.models().len(), 1);
    assert!(live.loading.is_empty(), "the promise is withdrawn");
}

/// A child whose name collides with a routed model cannot commit; the
/// promise is still withdrawn so the name is not a lingering 503.
#[cfg(all(feature = "local", feature = "test-fixtures"))]
#[tokio::test]
async fn a_commit_colliding_with_a_remote_name_fails_and_clears_loading() {
    let state = state();
    state
        .live
        .write()
        .await
        .loading
        .insert("alpha-model".to_owned());
    let child = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         [[endpoint]]\nid = \"local\"\nprotocol = \"openai\"\nbase_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
         [[model]]\nname = \"alpha-model\"\ndescription = \"collides\"\ncontext = 4096\nupstream = \"alpha-model\"\nendpoints = [\"local\"]\n",
    )
    .expect("child config parses");
    let routing = crate::routing::Routing::from_config(&child).expect("child routes");
    let runtime = crate::local::LocalRuntime::from_test_models(routing.models().to_vec());

    let error = super::commit(&state, &name("alpha"), runtime, Vec::new())
        .await
        .expect_err("a duplicate name cannot merge");

    assert!(
        matches!(&error, GatewayError::SwitchFailed { stage, .. } if *stage == "merge-routing"),
        "the merge names its stage: {error:?}"
    );
    let live = state.live.read().await;
    assert!(live.loading.is_empty());
    assert!(
        live.local.models().is_empty(),
        "the runtime is not installed"
    );
}
