//! Cancellation-aware publication of validated Gateway replacements.

use std::sync::TryLockError;
use std::time::Duration;

use super::{GatewayBinding, GatewayUpdater, build_snapshot};
use crate::gateway::GatewayError;

/// Cancellable replacement lock retry cadence.
const REPLACEMENT_RETRY_INTERVAL: Duration = Duration::from_millis(10);

impl GatewayBinding {
    fn replace_with_identity_cancellable(
        &self,
        base_url: &str,
        api_key: &str,
        identity: shared_sidecar::ValidatedConnection,
        cancellation: &shared_sidecar::CancellationToken,
    ) -> Result<bool, GatewayError> {
        self.replace_with_identity_cancellable_with_wait(
            base_url,
            api_key,
            identity,
            cancellation,
            shared_sidecar::CancellationToken::wait_timeout,
        )
    }

    pub(super) fn replace_with_identity_cancellable_with_wait(
        &self,
        base_url: &str,
        api_key: &str,
        identity: shared_sidecar::ValidatedConnection,
        cancellation: &shared_sidecar::CancellationToken,
        mut wait: impl FnMut(&shared_sidecar::CancellationToken, Duration) -> bool,
    ) -> Result<bool, GatewayError> {
        if cancellation.is_cancelled() {
            return Ok(false);
        }
        let snapshot = build_snapshot(base_url, api_key, 0, Some(identity))?;
        cancellation
            .run_if_active(|| {
                let _replacement = loop {
                    match self.replacement.try_lock() {
                        Ok(replacement) => break replacement,
                        Err(TryLockError::Poisoned(error)) => break error.into_inner(),
                        Err(TryLockError::WouldBlock) => {
                            if wait(cancellation, REPLACEMENT_RETRY_INTERVAL) {
                                return Ok(false);
                            }
                        }
                    }
                };
                if cancellation.is_cancelled() {
                    return Ok(false);
                }
                self.publish_snapshot(snapshot);
                Ok(true)
            })
            .unwrap_or(Ok(false))
    }
}

impl GatewayUpdater {
    /// Atomically replaces the local Gateway unless caller cancellation wins.
    ///
    /// Returns `Ok(false)` without publication when cancellation wins while
    /// another publisher owns the replacement lock.
    ///
    /// # Errors
    /// Returns [`GatewayError::Build`] if the replacement clients cannot
    /// initialize.
    pub fn replace_sidecar_cancellable(
        &self,
        connection: &shared_sidecar::ValidatedConnection,
        cancellation: &shared_sidecar::CancellationToken,
    ) -> Result<bool, GatewayError> {
        self.binding.replace_with_identity_cancellable(
            &format!("http://127.0.0.1:{}", connection.port()),
            connection.api_key(),
            connection.clone(),
            cancellation,
        )
    }
}
