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
//!    is alive, its process image is a `promptforge-gateway` binary (a
//!    reused pid cannot impersonate the gateway), `GET /health` answers
//!    200, and the file's bearer key is accepted on a key-gated route.
//!    Anything else is stale and the file is deleted.
//! 3. Launch races take [`launch_or_attach`]: the `gateway.json.lock`
//!    advisory lock elects one launcher; losers attach to the winner.
//! 4. A reader asks the gateway to exit with [`request_shutdown`], which
//!    posts the file's bearer key to `POST /shutdown`.
//!
//! URLs normalize to a literal `127.0.0.1`, never `localhost`, and probes
//! send the bound address as the `Host` header, matching the gateway's
//! loopback `Host` allowlist.

mod atomic;
mod error;
mod file;
mod health;
mod lock;
mod paths;
mod shutdown;
mod stale;
mod sys;

pub use crate::error::SidecarError;
pub use crate::file::{ConnectionFile, remove_if_mine};
pub use crate::health::{HealthError, ProbeError, wait_for_health};
pub use crate::lock::{LaunchDecision, LaunchLock, launch_or_attach};
pub use crate::paths::{
    CONNECTION_FILE_NAME, LOCK_FILE_NAME, connection_file_path, default_run_dir, lock_file_path,
    run_dir,
};
pub use crate::shutdown::{ShutdownError, request_shutdown};
#[cfg(feature = "test-fixtures")]
#[doc(hidden)]
pub use crate::stale::resolve_for_test;
pub use crate::stale::{Resolution, StaleReason, resolve};
