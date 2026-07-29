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
}

#[cfg(test)]
mod tests {
    use super::Tool;

    #[test]
    fn trait_is_dyn_compatible() {
        let tools: Vec<Box<dyn Tool>> = Vec::new();
        assert!(tools.is_empty());
    }
}
