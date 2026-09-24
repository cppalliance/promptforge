//! Shared harness for the facade suite's prepare cases.

use promptforge::RunContext;
use promptforge::timestamp::Timestamp;

/// A [`RunContext`] for the run `name` under the fixed host inputs every
/// fixture shares: the engine takes its seed and clock from the host, and
/// no fixture here asserts on the nonce or `sys.when`.
pub(super) fn context(name: impl Into<String>) -> RunContext {
    RunContext::new(name, 1, Timestamp::UNIX_EPOCH)
}
