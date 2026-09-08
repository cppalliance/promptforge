//! The shared sidecar discovery seam: the `gateway.json` connection file.
//!
//! The gateway writes `gateway.json` into the run directory
//! (`<home>/.promptforge/run`) after a successful bind, Jupyter-style: the
//! file carries the loopback port, the bearer key, the pid, the boot epoch,
//! the version, and the start time, so a reader (the workshop shell,
//! workshop-server) can attach to an already-running gateway instead of
//! launching a second one. The crate is synchronous and runtime-agnostic:
//! no tokio, axum, or reqwest.
//!
//! The flow:
//!
//! 1. The writer ([`ConnectionFile::write_to`]) lands the file atomically
//!    with owner-only permissions (mode `0600` on Unix; on Windows the file
//!    relies on the user profile's ACL, which already restricts it to the
//!    owner) and removes it on clean shutdown with [`remove_if_mine`].
//! 2. A reader ([`resolve`]) attaches only when the file is live: the pid
//!    is alive, one OS process boot with a `promptforge-gateway` image
//!    brackets a same-socket health and bearer proof, and the file carries
//!    a boot identity. Anything else is stale and the file is deleted.
//!    [`ValidatedConnection`] carries that point-in-time proof without
//!    exposing a forgeable constructor.
//! 3. Launch races take [`launch_or_attach`]: the `gateway.json.lock`
//!    advisory lock elects one parent launcher; losers attach to the winner.
//! 4. Every Gateway process holds [`GatewayInstanceLease`] from before
//!    startup side effects until process exit, independently of that parent
//!    launch election.
//! 5. A reader holding a [`ValidatedConnection`] asks the gateway to exit
//!    with [`request_shutdown`], which posts its bearer key to
//!    `POST /shutdown`.
//!
//! URLs normalize to a literal `127.0.0.1`, never `localhost`, and probes
//! send the bound address as the `Host` header, matching the gateway's
//! loopback `Host` allowlist.

mod atomic;
mod cancellation;
mod error;
mod file;
mod health;
mod lock;
mod paths;
mod shutdown;
mod stale;
mod sys;
mod validated;

pub use crate::cancellation::CancellationToken;
pub use crate::error::SidecarError;
pub use crate::file::{ConnectionFile, remove_if_mine};
pub use crate::health::{HealthError, ProbeError, wait_for_health, wait_for_health_cancellable};
pub use crate::lock::{
    GatewayInstanceLease, LaunchDecision, LaunchLock, launch_or_attach,
    launch_or_attach_cancellable,
};
pub use crate::paths::{
    CONNECTION_FILE_NAME, INSTANCE_LOCK_FILE_NAME, LOCK_FILE_NAME, connection_file_path,
    default_run_dir, instance_lock_file_path, lock_file_path, run_dir,
};
pub use crate::shutdown::{ShutdownError, request_shutdown};
pub use crate::stale::resolve_cancellable;
#[cfg(feature = "test-fixtures")]
#[doc(hidden)]
pub use crate::stale::resolve_for_test;
pub use crate::stale::{Resolution, StaleReason, is_running, resolve};
pub use crate::validated::{ValidatedConnection, ValidationError};
