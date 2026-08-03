//! The crate's integration tests, in one binary.
//!
//! What lives here is what only a real MCP session can show. A handler test
//! reaches the same code through `dispatch`, but it cannot say what a client
//! receives, and the progress path is defined by exactly that.

mod progress;
