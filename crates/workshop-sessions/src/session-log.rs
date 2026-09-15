//! The session log guard: logs the session's close when the connection
//! task ends, however it ends, so the session loop's exit paths carry no
//! cleanup calls.

/// Logs the session close when the connection task ends, however it ends,
/// so the session loop's exit paths carry no cleanup calls.
pub(super) struct SessionLog {
    pub(super) session: u64,
}

impl Drop for SessionLog {
    fn drop(&mut self) {
        tracing::info!(session = self.session, "chat session closed");
    }
}
