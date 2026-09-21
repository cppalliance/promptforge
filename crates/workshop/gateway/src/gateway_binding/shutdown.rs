//! Shutdown authority derived from a Gateway snapshot's validated identity.

use super::{GatewaySnapshot, GatewayUpdater};

impl GatewaySnapshot {
    /// Whether this generation is a supervised local sidecar: it holds
    /// a validated local Gateway boot, so the Workshop may shut it down
    /// and its supervisor relaunches the sibling. An explicitly configured
    /// endpoint (a LAN gateway) never is.
    #[must_use]
    pub fn is_sidecar(&self) -> bool {
        self.identity.is_some()
    }

    /// Requests shutdown from the validated local Gateway this snapshot
    /// names, posting the authenticated `POST /shutdown`.
    ///
    /// Returns `Ok(false)` without sending a request when the snapshot
    /// came from explicit configuration and therefore grants no local
    /// shutdown authority. The request is blocking I/O; an async caller
    /// runs it off the executor.
    ///
    /// # Errors
    /// Returns [`gateway_api_discovery::ShutdownError`] when the local Gateway
    /// refuses the request or cannot be reached.
    pub fn request_shutdown(&self) -> Result<bool, gateway_api_discovery::ShutdownError> {
        let Some(identity) = self.identity.as_ref() else {
            return Ok(false);
        };
        gateway_api_discovery::request_shutdown(identity)?;
        Ok(true)
    }
}

impl GatewayUpdater {
    /// Requests shutdown from the validated local Gateway in the current
    /// consumer snapshot, as that snapshot's
    /// [`GatewaySnapshot::request_shutdown`] does.
    ///
    /// Returns `Ok(false)` without sending a request when the current Gateway
    /// came from explicit configuration and therefore grants no local shutdown
    /// authority.
    ///
    /// # Errors
    /// Returns [`gateway_api_discovery::ShutdownError`] when the current local
    /// Gateway refuses the request or cannot be reached.
    pub fn request_shutdown(&self) -> Result<bool, gateway_api_discovery::ShutdownError> {
        self.binding.snapshot().request_shutdown()
    }
}
