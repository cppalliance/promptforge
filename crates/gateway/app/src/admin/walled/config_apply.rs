//! Apply and revert routes for pending config shadows:
//! `POST /admin/config-apply` and `POST /admin/config-revert`.
//!
//! Apply captures the pending state under the apply lock - a census of the
//! shadows, the parsed shadow-preferred config, and every shadow's current
//! contents - then releases the lock. A change that needs no reload (an env
//! shadow alone) is promoted inline. A config shadow runs as an
//! `ApplyConfig` command on the command queue: the command rebuilds the
//! remote routing table from the pending config, merges the running local
//! models under it, promotes the captured shadows under the apply lock, and
//! swaps the live routing, config, and web-search state in one write. A
//! failed or cancelled apply promotes nothing and leaves every shadow staged
//! for a retry. Sections the process reads once at boot (`[server]`,
//! `[workshop]`, `[[profile]]`, `[[local_model]]`, `[[stt_model]]`, `[stt]`)
//! and env shadows promote the same way but report `restart_required`: the
//! local runtime is fixed for the process lifetime. Revert cancels any apply
//! in flight, then deletes every shadow and touches nothing else. Saves, the
//! capture step, the commit, and revert serialize on one mutex, so apply only
//! captures combinations the latest save validated whole. Both routes reply
//! with plain JSON; the reload's `"Applying configuration"` text reaches
//! `GET /admin/progress` subscribers through the command's activity.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::http::Method;
use axum::routing::post;
use axum::{Json, Router};
use gateway_config::shadow_path;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::config_pending::{config_root, relative_name, shadow_census};
use crate::AppState;
use crate::auth::LoopbackCaller;
use crate::commands::Command;
use crate::commands::apply::{ApplyPlan, capture_apply, promote_captures};
use crate::error::error_chain;
use crate::error::{GatewayError, blocking};
use crate::registry::RouteInfo;

const APPLY: RouteInfo = RouteInfo::walled("/admin/config-apply", &[Method::POST]);
const REVERT: RouteInfo = RouteInfo::walled("/admin/config-revert", &[Method::POST]);

/// The apply and revert routes, as the registry sees them.
pub(crate) const ROUTES: &[RouteInfo] = &[APPLY, REVERT];

/// The apply and revert routes.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(APPLY.path, post(admin_config_apply))
        .route(REVERT.path, post(admin_config_revert))
}

/// The `POST /admin/config-apply` reply.
#[derive(Debug, Serialize)]
pub(crate) struct ApplyReply {
    /// The promoted real files, relative to the config root, sorted.
    applied: Vec<String>,
    /// Whether a config shadow applied and the remote routing reloaded.
    reloaded: bool,
    /// Whether a promoted change takes effect only at the next start.
    restart_required: bool,
}

/// The `POST /admin/config-revert` reply.
#[derive(Debug, Serialize)]
pub(crate) struct RevertReply {
    /// The deleted shadow files, relative to the config root.
    reverted: Vec<String>,
}

/// The `POST /admin/config-apply` route: bearer-authed, applies every
/// staged shadow, reloading the remote routing table when the change needs
/// it.
///
/// The reply is plain JSON - `{"applied": [...], "reloaded": bool,
/// "restart_required": bool}` - not SSE: the reload runs as a command on
/// the queue, so its `"Applying configuration"` text reaches
/// `GET /admin/progress` subscribers, and the response carries the outcome.
/// `applied` names the promoted real files relative to the config root,
/// sorted. `reloaded` is true when a config shadow applied successfully.
/// `restart_required` is true for an env shadow or a change to a section
/// the process reads once at boot: `[server]`, `[workshop]`, `[[profile]]`,
/// `[[local_model]]`, `[[stt_model]]`, or `[stt]`. With no shadows on disk
/// the reply is the clean no-op
/// `{"applied": [], "reloaded": false, "restart_required": false}`.
///
/// Nothing is promoted before the command commits. A parse failure replies
/// 500 before any command exists; a reload failure replies
/// [`GatewayError::ApplyReloadFailed`] (500) and a cancelled command -
/// the user's cancel, a revert, or shutdown - replies
/// [`GatewayError::ApplyCancelled`] (503). In both cases every shadow is
/// still staged, so a retry of Apply re-runs the whole thing.
pub(crate) async fn admin_config_apply(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
) -> Result<Json<ApplyReply>, GatewayError> {
    let config_path = crate::admin::config_path(&state)?.to_path_buf();
    let (enqueued, applied, restart_required) = {
        // The lock spans the census, the parse, and the capture (or the
        // inline promotion), so a save cannot land between them and the
        // snapshot is one the latest save validated whole. It is released
        // before the command runs: the queue serializes the reload itself.
        let _guard = state.apply.lock().await;
        let plan = blocking(move || capture_apply(&config_path)).await??;
        let snapshot = match plan {
            ApplyPlan::Inline {
                files,
                restart_required,
            } => {
                let applied = blocking(move || promote_captures(&files)).await??;
                return Ok(Json(ApplyReply {
                    applied,
                    reloaded: false,
                    restart_required,
                }));
            }
            ApplyPlan::Reload(snapshot) => snapshot,
        };
        let applied = snapshot.applied_names();
        let restart_required = snapshot.restart_required;
        let enqueued = state.commands.enqueue(Command::ApplyConfig {
            snapshot,
            token: CancellationToken::new(),
        });
        (enqueued, applied, restart_required)
    };
    let outcome = enqueued.outcome.await.unwrap_or_else(|_| {
        // The worker settles every command it begins, so a dropped sender
        // means the worker task itself died.
        Arc::new(Err(GatewayError::switch_failed(
            "queue",
            std::io::Error::other("the command queue dropped the command without settling it"),
        )))
    });
    match &*outcome {
        Ok(_) => Ok(Json(ApplyReply {
            applied,
            reloaded: true,
            restart_required,
        })),
        Err(GatewayError::CommandCancelled(_)) => Err(GatewayError::ApplyCancelled),
        Err(error) => Err(GatewayError::ApplyReloadFailed(error_chain(error))),
    }
}

/// The `POST /admin/config-revert` route: bearer-authed, cancels any apply
/// in flight, deletes every shadow file, and touches nothing else.
///
/// The reply is `{"reverted": [...]}` naming the deleted shadow files
/// relative to the config root, sorted. The real files were never touched
/// by a save, so nothing is rewritten: deleting the shadows is the whole
/// revert. An apply cancelled here settles its route with
/// [`GatewayError::ApplyCancelled`].
pub(crate) async fn admin_config_revert(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
) -> Result<Json<RevertReply>, GatewayError> {
    // A revert issued during an apply wins: cancel the apply before its
    // commit can write the snapshot over the files being reverted. The
    // commit re-checks the token under the apply lock, so an apply already
    // waiting for that lock still stops.
    state.commands.cancel_apply();
    // The same guard as apply's capture and commit: a revert must not race
    // either.
    let _guard = state.apply.lock().await;
    let config_path = crate::admin::config_path(&state)?.to_path_buf();
    let reverted = blocking(move || delete_all_shadows(&config_path)).await??;
    Ok(Json(RevertReply { reverted }))
}

/// Deletes every shadow the census finds, returning the deleted shadow
/// files relative to the config root, sorted.
fn delete_all_shadows(config_path: &Path) -> Result<Vec<String>, GatewayError> {
    let census = shadow_census(config_path)?;
    let root = config_root(config_path);
    let mut reverted: Vec<String> = Vec::with_capacity(census.files.len());
    for file in &census.files {
        let shadow: PathBuf = shadow_path(file);
        std::fs::remove_file(&shadow)
            .map_err(|source| GatewayError::ConfigWriteIo(Box::new(source)))?;
        reverted.push(relative_name(&shadow, root));
    }
    reverted.sort_unstable();
    Ok(reverted)
}

#[cfg(test)]
#[path = "config_apply-tests.rs"]
mod tests;
