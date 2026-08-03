//! The crate's integration tests, in one binary.
//!
//! What lives here is what only a real MCP session, a real process, or a real
//! repository file can show. A handler test reaches the same code through
//! `dispatch`, but it cannot say what a client receives, and the progress path
//! is defined by exactly that; a unit test writes its own prompts into a
//! temporary directory, so nothing but `shipped` reads what the repository
//! actually ships.

mod progress;
mod shipped;
mod stdio;
mod watch;
