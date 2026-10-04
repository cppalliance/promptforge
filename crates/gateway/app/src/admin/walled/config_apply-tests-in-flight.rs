//! Applies while the command is in flight: concurrent applies sharing one
//! command, an apply behind the boot load, cancellation, a save landing
//! mid-apply, and a revert during an active apply.

use std::time::Duration;

use gateway_config::ProfileName;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::commands::Command;
use crate::error::GatewayError;
use crate::park::{Phase, PhasePark};
use crate::test_support::wait_until;

/// [`serve_fixture`] with the apply command parked at its commit, so a
/// test can act between the capture and the promotion.
async fn serve_parked_fixture(
    config: Config,
    paths: AdminPaths,
) -> (SocketAddr, AppState, Arc<PhasePark>) {
    let mut state = app_state(config, Some(paths));
    let park = Arc::new(PhasePark::at(Phase::ApplyCommit));
    state.park = Some(Arc::clone(&park));
    let _worker = state.commands.spawn_worker(&state).expect("worker spawns");
    let addr = serve_state(state.clone()).await;
    (addr, state, park)
}

/// Asserts the apply reply is the cancellation envelope the config UI
/// keys on.
async fn assert_apply_cancelled(response: reqwest::Response) {
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = response.json().await.expect("error envelope");
    assert_eq!(body["error"]["code"], "apply_cancelled");
    assert_eq!(body["error"]["type"], "server_error");
    assert_eq!(
        body["error"]["message"],
        GatewayError::ApplyCancelled.to_string()
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("still staged")),
        "the message tells the user their changes survive: {body}"
    );
}

/// Two applies in flight at once share one command: the second attaches
/// to the first through the debounce, both replies report the same
/// `applied` list, and the reload runs exactly once.
#[tokio::test]
async fn concurrent_applies_promote_the_pending_config_once() {
    let (_temp, config, paths) = fixture();
    let config_path = paths.config_path.clone();
    let (addr, state, park) = serve_parked_fixture(config, paths).await;
    stage_gamma(&config_path);

    let first = tokio::spawn(post(addr, "admin/config-apply"));
    park.entered().await;
    assert_eq!(
        state.hub.current().text,
        "Applying configuration",
        "the parked apply holds the command's activity with its stage text"
    );
    let second = tokio::spawn(post(addr, "admin/config-apply"));
    wait_until("the second apply to attach to the first", || {
        state.commands.active_waiters() == 2
    })
    .await;
    assert!(
        state.commands.pending_commands().is_empty(),
        "the second apply attached to the active one instead of queueing"
    );
    park.release();

    let first = first.await.expect("first apply task");
    let second = second.await.expect("second apply task");
    assert_eq!(first.status(), reqwest::StatusCode::OK);
    assert_eq!(second.status(), reqwest::StatusCode::OK);
    let first: serde_json::Value = first.json().await.expect("first body");
    let second: serde_json::Value = second.json().await.expect("second body");
    let expected = serde_json::json!(["gateway.toml"]);
    assert_eq!(first["applied"], expected);
    assert_eq!(
        second["applied"], expected,
        "both replies report the shared outcome"
    );
    assert_eq!(first["reloaded"], true);
    assert_eq!(second["reloaded"], true);
    assert!(
        !state.hub.current().busy,
        "the one command settled and released its activity"
    );
    assert!(!shadow_path(&config_path).exists());
    assert!(routes(&state, "gamma-model").await);
}

/// An apply enqueued after the boot `LoadProfile` never displaces it:
/// the boot load settles on its own terms over the production worker,
/// then the apply runs over the table it published and completes. The
/// queue's FIFO rule under an active boot load is pinned in
/// `commands.rs`.
#[tokio::test]
async fn an_apply_after_the_boot_load_reloads_over_the_published_table() {
    let (_temp, config, paths) = fixture();
    let config_path = paths.config_path.clone();
    let (addr, state) = serve_fixture(config, paths).await;
    stage_gamma(&config_path);

    let boot = state.commands.enqueue(Command::load_profile(
        ProfileName::parse("alpha").expect("profile name"),
        CancellationToken::new(),
    ));
    wait_until("the boot load to settle", || {
        state.commands.active_command().is_none()
    })
    .await;
    let outcome = tokio::time::timeout(Duration::from_secs(10), boot.outcome)
        .await
        .expect("the boot load settles")
        .expect("the boot load settles with an outcome");
    assert!(outcome.is_ok(), "a remote-only profile loads: {outcome:?}");

    let response = post(addr, "admin/config-apply").await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let reply: serde_json::Value = response.json().await.expect("apply body");
    assert_eq!(reply["reloaded"], true);
    assert_eq!(reply["applied"], serde_json::json!(["gateway.toml"]));
    assert!(!shadow_path(&config_path).exists());
    assert!(routes(&state, "gamma-model").await);
    assert_eq!(live_profile(&state).await.as_deref(), Some("alpha"));
}

/// A cancelled apply promotes nothing: the shadow stays on disk with its
/// contents, the dirty report is unchanged, the reply is the
/// cancellation envelope, and a retry applies the same change.
#[tokio::test]
async fn a_cancelled_apply_leaves_every_shadow_staged_and_a_retry_succeeds() {
    let (_temp, config, paths) = fixture();
    let config_path = paths.config_path.clone();
    let original_config = std::fs::read_to_string(&config_path).expect("read config");
    let (addr, state, park) = serve_parked_fixture(config, paths).await;
    stage_gamma(&config_path);
    let staged = std::fs::read_to_string(shadow_path(&config_path)).expect("staged shadow");
    let dirty_before = get_json(addr, "admin/config-dirty").await;
    assert_eq!(dirty_before["dirty"], true);

    let apply = tokio::spawn(post(addr, "admin/config-apply"));
    park.entered().await;
    assert!(state.commands.cancel_active());
    park.release();

    assert_apply_cancelled(apply.await.expect("apply task")).await;
    assert_eq!(
        std::fs::read_to_string(shadow_path(&config_path)).expect("config shadow"),
        staged,
        "the config shadow is still staged"
    );
    assert_eq!(
        std::fs::read_to_string(&config_path).expect("re-read config"),
        original_config,
        "nothing was promoted"
    );
    assert_eq!(
        get_json(addr, "admin/config-dirty").await,
        dirty_before,
        "the dirty report is unchanged"
    );
    assert!(!routes(&state, "gamma-model").await, "nothing went live");

    // The retry parks at the same phase; a stored release lets it through.
    park.release();
    let retry = post(addr, "admin/config-apply").await;
    assert_eq!(retry.status(), reqwest::StatusCode::OK);
    let reply: serde_json::Value = retry.json().await.expect("retry body");
    assert_eq!(reply["reloaded"], true);
    assert_eq!(reply["applied"], serde_json::json!(["gateway.toml"]));
    assert!(!shadow_path(&config_path).exists());
    assert!(routes(&state, "gamma-model").await);
}

/// A save that lands mid-apply neither blocks nor is lost: the snapshot's
/// contents land in the real file, and the newer shadow stays pending as
/// the next change.
#[tokio::test]
async fn a_save_landing_mid_apply_stays_pending_while_the_snapshot_lands() {
    let (_temp, config, paths) = fixture();
    let config_path = paths.config_path.clone();
    let (addr, state, park) = serve_parked_fixture(config, paths).await;
    stage_gamma(&config_path);

    let apply = tokio::spawn(post(addr, "admin/config-apply"));
    park.entered().await;
    let save = tokio::time::timeout(
        Duration::from_secs(10),
        save_edited(addr, |body| {
            body["model"][0]["description"] = serde_json::json!("edited mid-apply");
        }),
    )
    .await
    .expect("the save completes while the apply is active");
    assert_eq!(save.status(), reqwest::StatusCode::OK);
    park.release();

    let response = apply.await.expect("apply task");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let reply: serde_json::Value = response.json().await.expect("apply body");
    assert_eq!(reply["applied"], serde_json::json!(["gateway.toml"]));
    let real = std::fs::read_to_string(&config_path).expect("read config");
    assert!(
        real.contains("gamma-model") && !real.contains("edited mid-apply"),
        "the snapshot's contents landed in the real file"
    );
    let pending = std::fs::read_to_string(shadow_path(&config_path)).expect("config shadow");
    assert!(
        pending.contains("edited mid-apply"),
        "the newer save stays pending instead of being deleted"
    );
    assert!(routes(&state, "gamma-model").await);
    let dirty = get_json(addr, "admin/config-dirty").await;
    assert_eq!(dirty["pending_files"], serde_json::json!(["gateway.toml"]));
}

/// A revert during an active apply wins: the apply settles as cancelled,
/// its commit writes nothing, and the shadow is gone.
#[tokio::test]
async fn a_revert_during_an_active_apply_cancels_it_and_the_commit_writes_nothing() {
    let (_temp, config, paths) = fixture();
    let config_path = paths.config_path.clone();
    let original_config = std::fs::read_to_string(&config_path).expect("read config");
    let (addr, state, park) = serve_parked_fixture(config, paths).await;
    stage_gamma(&config_path);

    let apply = tokio::spawn(post(addr, "admin/config-apply"));
    park.entered().await;

    let revert = post(addr, "admin/config-revert").await;
    assert_eq!(revert.status(), reqwest::StatusCode::OK);
    let reply: serde_json::Value = revert.json().await.expect("revert body");
    assert_eq!(reply["reverted"], serde_json::json!(["gateway.toml.next"]));
    park.release();

    assert_apply_cancelled(apply.await.expect("apply task")).await;
    assert_eq!(
        std::fs::read_to_string(&config_path).expect("re-read config"),
        original_config,
        "the cancelled apply's commit wrote nothing"
    );
    assert!(!shadow_path(&config_path).exists());
    assert!(!routes(&state, "gamma-model").await);
    assert_eq!(live_profile(&state).await.as_deref(), Some("alpha"));
    wait_until("the queue to go idle", || {
        state.commands.active_command().is_none()
    })
    .await;
}
