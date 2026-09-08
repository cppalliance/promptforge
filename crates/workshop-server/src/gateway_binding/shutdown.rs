//! Shutdown authority derived from the current Gateway snapshot.

use super::GatewayUpdater;

impl GatewayUpdater {
    /// Requests shutdown from the validated local Gateway in the current
    /// consumer snapshot.
    ///
    /// Returns `Ok(false)` without sending a request when the current Gateway
    /// came from explicit configuration and therefore grants no local shutdown
    /// authority.
    ///
    /// # Errors
    /// Returns [`shared_sidecar::ShutdownError`] when the current local
    /// Gateway refuses the request or cannot be reached.
    pub fn request_shutdown(&self) -> Result<bool, shared_sidecar::ShutdownError> {
        let snapshot = self.binding.snapshot();
        let Some(identity) = snapshot.identity.as_ref() else {
            return Ok(false);
        };
        shared_sidecar::request_shutdown(identity)?;
        Ok(true)
    }
}
