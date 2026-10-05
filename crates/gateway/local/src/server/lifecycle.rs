//! Respawn and bounded teardown of the guarded `llama-server` child.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::Instant;

use super::support::{display_invocation, new_capture, server_args};
use super::{
    LaunchOptions, Result, ServerGuard, SpawnRequest, TEARDOWN_DEADLINE, TEARDOWN_POLL, WaitOutcome,
};
use crate::error::LocalError;

impl ServerGuard {
    /// Returns whether the child process is still running.
    ///
    /// When the child has already exited, joins capture threads so a later
    /// [`Self::respawn`] can attach fresh readers.
    pub(crate) fn is_running(&mut self) -> Result<bool> {
        if self.child_status()?.is_none() {
            Ok(true)
        } else {
            self.join_readers_checked()?;
            Ok(false)
        }
    }

    /// Kills the current child (if any) and starts a new one on the same port,
    /// alias, and API key, then waits until authenticated readiness succeeds.
    ///
    /// `cancel` is polled during the readiness wait: when it is set (an explicit
    /// teardown at profile-switch time), the respawn aborts promptly with
    /// [`LocalError::StartupInterrupted`] instead of waiting out the readiness
    /// deadline, so teardown never blocks behind an in-flight respawn
    /// (PF-GW-SERVER-004).
    ///
    /// # Errors
    /// Returns a [`LocalError`] when kill, spawn, readiness, or cancellation fails.
    pub(crate) fn respawn(
        &mut self,
        executable: &Path,
        model: &Path,
        options: &LaunchOptions,
        cancel: &AtomicBool,
    ) -> Result<()> {
        self.terminate_child()?;
        self.join_readers_checked()?;

        let args = server_args(
            model,
            self.port,
            &self.model_alias,
            self.api_key.expose(),
            options,
        );
        let request = SpawnRequest {
            executable,
            args: &args,
            path_prefix: &options.path_prefix,
            #[cfg(test)]
            port: self.port,
            #[cfg(test)]
            model_alias: &self.model_alias,
            #[cfg(test)]
            api_key: self.api_key.expose(),
        };
        let child = self.spawner.spawn(&request)?;
        self.child = child;
        self.stdout = new_capture();
        self.stderr = new_capture();
        self.readers = Vec::with_capacity(2);
        self.start_capture()?;

        let policy = self.policy;
        match self.wait_until_ready(cancel, policy)? {
            WaitOutcome::Ready => Ok(()),
            WaitOutcome::PortCollision(status) => Err(LocalError::RespawnPortCollision {
                port: self.port,
                detail: format!(
                    "child exited with {status}\n{}\n{}",
                    display_invocation(executable, &args),
                    self.diagnostics()
                ),
            }),
        }
    }

    /// Best-effort join used only by `Drop`: a reader panic or read error is
    /// intentionally discarded because there is no caller to report to.
    fn join_readers(&mut self) {
        for (_stream, reader) in self.readers.drain(..) {
            let _ignored = reader.join();
        }
    }

    /// Joins the capture readers and surfaces the first read error or panic.
    ///
    /// Used from the checked lifecycle paths (`is_running`, `respawn`,
    /// `classify_early_exit`) so a genuine capture read failure is returned to
    /// the caller instead of being erased (SERVER-005). Normal completion is an
    /// EOF (`Ok(())`) when the child's pipes close.
    pub(super) fn join_readers_checked(&mut self) -> Result<()> {
        let mut first_error: Option<LocalError> = None;
        for (stream, reader) in self.readers.drain(..) {
            match reader.join() {
                Ok(Ok(())) => {}
                Ok(Err(source)) => {
                    first_error.get_or_insert(LocalError::CaptureRead { stream, source });
                }
                Err(_) => {
                    first_error.get_or_insert(LocalError::CapturePanic { stream });
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Explicit, bounded teardown: terminate the child and join capture readers.
    ///
    /// Used by [`crate::upstream::LocalUpstream::shutdown`] to free the
    /// child deterministically at profile-switch time, when dropping the runtime
    /// alone would not (routing still holds `Arc<dyn Upstream>` clones).
    ///
    /// # Errors
    /// Returns a [`LocalError`] when kill/reap or a capture reader fails.
    pub(crate) fn shutdown(&mut self) -> Result<()> {
        self.terminate_child()?;
        self.join_readers_checked()
    }

    /// Best-effort bounded termination of the current child.
    ///
    /// Checks `try_wait` first so an already-exited child is never re-signalled,
    /// then kills and reaps within [`TEARDOWN_DEADLINE`] so teardown can never
    /// block unbounded. Kill and reap-timeout failures are surfaced to callers.
    fn terminate_child(&mut self) -> Result<()> {
        if self.child_status()?.is_some() {
            return Ok(());
        }
        self.child
            .kill()
            .map_err(|source| LocalError::Kill { source })?;
        let deadline = Instant::now() + TEARDOWN_DEADLINE;
        loop {
            if self.child_status()?.is_some() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(LocalError::TeardownTimeout);
            }
            thread::sleep(TEARDOWN_POLL);
        }
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        // Best-effort, bounded teardown: `terminate_child` caps its reap at
        // `TEARDOWN_DEADLINE`, so drop never waits unbounded. Explicit teardown
        // with error reporting is `shutdown`; here the result is discarded
        // because Drop has no caller to report to (SERVER-001/005).
        let _ignored = self.terminate_child();
        self.join_readers();
    }
}
