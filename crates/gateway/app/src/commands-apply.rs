//! The `ApplyConfig` command: what an apply captures, and how the command
//! commits it.
//!
//! The `POST /admin/config-apply` route takes the census under
//! the apply lock and calls [`capture_apply`] to turn it into an
//! [`ApplyPlan`]: an inline promotion for shadows that need no reload, or
//! an [`ApplySnapshot`] that is queued as `Command::ApplyConfig` and
//! runs through [`apply_config`]. The snapshot vocabulary and the
//! promotion step sit here beside the command that consumes them, so the
//! route module holds only the two handlers and their replies.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gateway_config::{Config, ProfileSelection, load_pending_config, shadow_path, write_atomic};
use gateway_progress::Activity;
#[cfg(feature = "web-search")]
use gateway_web_search::WebSearchState;
use tokio_util::sync::CancellationToken;

use super::{APPLY_CONFIG_LABEL, Outcome};
use crate::AppState;
use crate::config_shadow::{canonical_form, config_root, relative_name, shadow_census};
use crate::error::{GatewayError, blocking, config_write_error};
use crate::routing::Routing;

/// Top-level sections the process reads once at boot. A change to one of
/// them promotes to disk but takes effect at the next start, so the apply
/// reports `restart_required`.
const RESTART_SECTIONS: [&str; 6] = [
    "server",
    "workshop",
    "profile",
    "local_model",
    "stt_model",
    "stt",
];

/// One shadow as the Apply route captured it, ready to land in its real
/// file at the command's commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShadowCapture {
    /// The real file the shadow stands in for, in canonical form.
    pub(crate) real_path: PathBuf,
    /// The real file rendered for the wire, relative to the config root.
    pub(crate) relative_name: String,
    /// The shadow's contents at capture time.
    pub(crate) contents: String,
}

/// What one reloading apply puts onto the command queue: the parsed
/// pending config and every captured shadow.
#[derive(Debug)]
pub(crate) struct ApplySnapshot {
    /// The shadow-preferred pending config, parsed and validated, with no
    /// profile selected (`active_profile()` is `None` and the local and
    /// speech-to-text subsets are empty): the apply swaps the remote
    /// catalog and never the local runtime. Boxed so the `Command` enum
    /// stays the size of its other variants.
    pub(crate) config: Box<Config>,
    /// Every shadow the census found, with its contents at capture time:
    /// the config shadow and any env shadow.
    pub(crate) files: Vec<ShadowCapture>,
    /// Whether an env or boot-read setting changed.
    pub(crate) restart_required: bool,
}

impl ApplySnapshot {
    /// The captured real files rendered for the wire, sorted.
    pub(crate) fn applied_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .files
            .iter()
            .map(|file| file.relative_name.clone())
            .collect();
        names.sort_unstable();
        names
    }
}

/// What the census decided: promote inline, or reload through the queue.
pub(crate) enum ApplyPlan {
    /// No config shadow: the captures are promoted under the route's lock
    /// and no command runs.
    Inline {
        /// The captures to promote.
        files: Vec<ShadowCapture>,
        /// Whether a promoted change takes effect only at the next start.
        restart_required: bool,
    },
    /// A config shadow: the reload runs as an `ApplyConfig` command and
    /// promotes the captures at its commit.
    Reload(ApplySnapshot),
}

/// Takes the census, parses the pending config when a reload is needed, and
/// reads every shadow's contents. Touches no real file.
pub(crate) fn capture_apply(config_path: &Path) -> Result<ApplyPlan, GatewayError> {
    let census = shadow_census(config_path)?;
    let root = config_root(config_path);
    let config_canonical = canonical_form(config_path);
    let env_canonical = canonical_form(&config_path.with_extension("env"));
    let needs_reload = census.files.iter().any(|file| file == &config_canonical);
    let mut restart_required = census
        .sections
        .iter()
        .any(|section| RESTART_SECTIONS.contains(&section.as_str()));
    let mut files = Vec::with_capacity(census.files.len());
    for file in &census.files {
        if file == &env_canonical {
            restart_required = true;
        }
        let shadow = shadow_path(file);
        let contents = std::fs::read_to_string(&shadow)
            .map_err(|source| GatewayError::ConfigWriteIo(Box::new(source)))?;
        files.push(ShadowCapture {
            real_path: file.clone(),
            relative_name: relative_name(file, root),
            contents,
        });
    }
    if !needs_reload {
        return Ok(ApplyPlan::Inline {
            files,
            restart_required,
        });
    }
    // The selection is irrelevant to what the apply swaps (the remote
    // catalog is the same for every profile). The pending loader resolves
    // the state file the way the next boot would, which admits a stale or
    // absent one, and the selection it resolved is then dropped: a
    // persisted name can differ from the running profile (a switch that
    // persisted a new name and is waiting on a restart) and must not be
    // published as the live document's selection.
    let config = load_pending_config(config_path, &ProfileSelection::default())
        .and_then(|config| config.select_profile(None))
        .map_err(config_write_error)?;
    Ok(ApplyPlan::Reload(ApplySnapshot {
        config: Box::new(config),
        files,
        restart_required,
    }))
}

/// Lands every capture in its real file and retires the shadows it came
/// from. The caller holds the apply lock.
///
/// For each capture the real file is replaced atomically with the captured
/// contents, then the shadow that exists now is compared against them: an
/// equal shadow is deleted (promotion complete), a different one - a save
/// landed since the capture - stays in place as the next pending change,
/// and a missing one needs nothing. The two invariants this keeps exact:
/// the real files always equal what is live, and a shadow always means
/// "not yet applied". Returns the promoted real files rendered for the
/// wire, sorted.
pub(crate) fn promote_captures(captures: &[ShadowCapture]) -> Result<Vec<String>, GatewayError> {
    let mut applied = Vec::with_capacity(captures.len());
    for capture in captures {
        write_atomic(&capture.real_path, &capture.contents).map_err(config_write_error)?;
        let shadow = shadow_path(&capture.real_path);
        match std::fs::read_to_string(&shadow) {
            Ok(current) if current == capture.contents => {
                if let Err(source) = std::fs::remove_file(&shadow)
                    && source.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(GatewayError::ConfigWriteIo(Box::new(source)));
                }
            }
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(GatewayError::ConfigWriteIo(Box::new(source))),
        }
        applied.push(capture.relative_name.clone());
    }
    applied.sort_unstable();
    Ok(applied)
}

/// The `ApplyConfig` command body: under the `"Applying configuration"`
/// text, swaps the remote routing table live and promotes the captured
/// shadows. The activity drops with the return on every path.
///
/// Any failure under a fired token reports as the cancellation it is, so
/// the route's reply can promise the shadows are still staged.
pub(crate) async fn apply_config(
    state: &AppState,
    snapshot: ApplySnapshot,
    token: CancellationToken,
    activity: Activity,
) -> Outcome {
    let ApplySnapshot { config, files, .. } = snapshot;
    activity.set_text("Applying configuration");
    match apply_snapshot(state, *config, files, &token).await {
        Ok(summary) => Ok(summary),
        Err(_) if token.is_cancelled() => Err(apply_cancelled()),
        Err(error) => Err(error),
    }
}

fn apply_cancelled() -> GatewayError {
    GatewayError::CommandCancelled(APPLY_CONFIG_LABEL.to_owned())
}

/// Builds the new routing table, then commits under the apply lock: the
/// captures land in their real files first (a failed promotion changes
/// nothing live), and one live write swaps the routing, config, and
/// web-search state. The running local children are never touched; their
/// routing entries persist under the new remote catalog.
async fn apply_snapshot(
    state: &AppState,
    config: Config,
    files: Vec<ShadowCapture>,
    token: &CancellationToken,
) -> Outcome {
    if token.is_cancelled() {
        return Err(apply_cancelled());
    }
    let remote = Routing::from_config(&config)
        .map_err(|error| GatewayError::switch_failed("build-routing", error))?;
    // Only queue commands change the local runtime, and this is one, so the
    // set read here is the set the swap below publishes.
    #[cfg(feature = "local")]
    let routing = {
        let live = state.live.read().await;
        remote
            .merge(live.local.models().iter().cloned())
            .map_err(|error| GatewayError::switch_failed("merge-routing", error))?
    };
    #[cfg(not(feature = "local"))]
    let routing = remote;
    #[cfg(feature = "web-search")]
    let web_search = config
        .web_search_config()
        .map(WebSearchState::new)
        .map(Arc::new);
    #[cfg(test)]
    state.park_at(crate::park::Phase::ApplyCommit).await;
    // The commit holds the apply lock so no save, revert, or pending read
    // interleaves with the promotion and the live swap. A revert fires the
    // token before taking this lock, so the re-check under it is what keeps
    // a cancelled apply from writing over files the user just reverted.
    let _publication = tokio::select! {
        biased;
        () = token.cancelled() => return Err(apply_cancelled()),
        guard = state.apply.lock() => guard,
    };
    if token.is_cancelled() {
        return Err(apply_cancelled());
    }
    let applied = blocking(move || promote_captures(&files)).await??;
    let mut live = state.live.write().await;
    live.routing = Arc::new(routing);
    live.config = Arc::new(config);
    #[cfg(feature = "web-search")]
    {
        live.web_search = web_search;
    }
    Ok(format!("applied {}", applied.join(", ")))
}
