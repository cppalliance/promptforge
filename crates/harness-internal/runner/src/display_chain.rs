//! Rendering an error and its cause chain as one line of text for a
//! person or a model.
//!
//! A `thiserror` variant renders only its own message; its `#[source]` is
//! reachable through `source()` but not repeated in the text. Where the
//! text leaves the program - a run's failed outcome in the log, a
//! failure pushed to a session's client - the chain is walked here so the
//! reader sees the cause and not just the outermost frame.

use std::error::Error;

/// Renders `error`'s text followed by each cause in its `source()` chain,
/// separated by `: `.
///
/// A cause whose text the accumulated rendering already contains is
/// skipped: some variants copy their source's text into their own
/// message (an engine `LuaRuntime { message, source }`, for one), and
/// appending that cause again would print it twice. The check is a plain
/// substring test on the text rendered so far.
#[must_use]
pub fn display_chain(error: &dyn Error) -> String {
    let mut rendered = error.to_string();
    let mut cause = error.source();
    while let Some(current) = cause {
        let text = current.to_string();
        if !text.is_empty() && !rendered.contains(&text) {
            rendered.push_str(": ");
            rendered.push_str(&text);
        }
        cause = current.source();
    }
    rendered
}

#[cfg(test)]
#[path = "display_chain-tests.rs"]
mod tests;
