//! Shared test support for the facade suite's prepare cases.

use promptforge::RunContext;
use promptforge::timestamp::Timestamp;

/// A [`RunContext`] for the run `name` under the fixed Harness inputs
/// every fixture shares: the Engine takes its seed and clock from the
/// Harness, and no fixture here asserts on the nonce or `sys.when`.
pub(super) fn context(name: impl Into<String>) -> RunContext {
    RunContext::new(name, 1, Timestamp::UNIX_EPOCH)
}
