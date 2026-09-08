//! Cancellation-aware publication of validated Gateway replacements.

use std::sync::TryLockError;
use std::time::Duration;

use super::{GatewayBinding, GatewayPublicationError, GatewayUpdater, build_snapshot};

/// Cancellable replacement lock retry cadence.
const REPLACEMENT_RETRY_INTERVAL: Duration = Duration::from_millis(10);

impl GatewayBinding {
    fn replace_with_identity_cancellable(
        &self,
        base_url: &str,
        api_key: &str,
        identity: shared_sidecar::ValidatedConnection,
        cancellation: &shared_sidecar::CancellationToken,
    ) -> Result<bool, GatewayPublicationError> {
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
    ) -> Result<bool, GatewayPublicationError> {
        if cancellation.is_cancelled() {
            return if self.publication_closed() {
                Err(GatewayPublicationError::PublicationClosed)
            } else {
                Ok(false)
            };
        }
        let snapshot = build_snapshot(base_url, api_key, 0, Some(identity))?;
        cancellation
            .run_if_active(|| {
                let mut publication = loop {
                    match self.publication.try_lock() {
                        Ok(publication) => break publication,
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
                if publication.closed {
                    return Err(GatewayPublicationError::PublicationClosed);
                }
                let generation = publication.next_generation;
                publication.next_generation = publication.next_generation.saturating_add(1);
                let mut snapshot = snapshot;
                snapshot.generation = generation;
                self.current.store(std::sync::Arc::new(snapshot));
                self.changed.send_replace(generation);
                Ok(true)
            })
            .unwrap_or_else(|| {
                if self.publication_closed() {
                    Err(GatewayPublicationError::PublicationClosed)
                } else {
                    Ok(false)
                }
            })
    }
}

impl GatewayUpdater {
    /// Atomically replaces the local Gateway unless caller cancellation wins.
    ///
    /// Returns `Ok(false)` without publication when cancellation wins while
    /// another publisher owns the replacement lock.
    ///
    /// # Errors
    /// Returns [`GatewayPublicationError::Build`] if the replacement clients
    /// cannot initialize, or
    /// [`GatewayPublicationError::PublicationClosed`] after teardown revokes
    /// replacement publication.
    pub fn replace_sidecar_cancellable(
        &self,
        connection: &shared_sidecar::ValidatedConnection,
        cancellation: &shared_sidecar::CancellationToken,
    ) -> Result<bool, GatewayPublicationError> {
        self.binding.replace_with_identity_cancellable(
            &format!("http://127.0.0.1:{}", connection.port()),
            connection.api_key(),
            connection.clone(),
            cancellation,
        )
    }
}
