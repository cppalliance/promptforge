//! Tools the executor can dispatch during a model's tool-call loop.
//!
//! Some tools run locally in this process (for example fetching and rendering a
//! web page), while others proxy through the gateway so a shared credential
//! never leaves the server. Both kinds share the [`Tool`] trait so the executor
//! can dispatch them uniformly by their wire name.

use crate::Result;

pub mod web_search;

pub use web_search::WebSearch;

/// A tool the executor can dispatch during a model's tool-call loop.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// The tool's wire name, matching the prompt frontmatter and the model's
    /// tool-call name.
    fn name(&self) -> &str;

    /// A one-sentence description supplied to the model.
    fn description(&self) -> &str;

    /// The JSON Schema describing the tool's parameters.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Execute the tool with the given JSON arguments and return its result.
    ///
    /// # Errors
    /// Returns [`crate::Error`] if the tool call fails (bad arguments, network,
    /// or a backend failure).
    async fn call(&self, args: serde_json::Value) -> Result<String>;

    /// Whether this tool's result is untrusted external data.
    ///
    /// A tool that returns `true` has its result wrapped in a self-contained
    /// guard block (a data-not-instructions rule plus a random-tagged,
    /// escape-protected delimiter) before it reaches the model, reducing the
    /// risk that attacker-controlled content (for example a fetched web page)
    /// is followed as instructions. Defaults to `false`, so a tool is trusted
    /// and its result appended verbatim unless it opts in; the default means
    /// existing implementations need no change.
    fn untrusted_output(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{Tool, WebSearch};

    #[test]
    fn trait_is_dyn_compatible() {
        let tools: Vec<Box<dyn Tool>> = Vec::new();
        assert!(tools.is_empty());
    }

    #[test]
    fn trusted_tool_defaults_to_not_untrusted() {
        // A tool that does not opt in (here the structured-snippet web search)
        // inherits the defaulted `false`, so its result is appended verbatim.
        assert!(!WebSearch::new("http://localhost", "test").untrusted_output());
    }
}
