//! Tools the executor can dispatch during a model's tool-call loop.
//!
//! Some tools run locally in this process (for example fetching and rendering a
//! web page), while others proxy through the gateway so a shared credential
//! never leaves the server. Both kinds share the [`Tool`] trait so the executor
//! can dispatch them uniformly. Stable identity is separate from the wire name
//! used by the current model transport.

use std::sync::Arc;

use crate::Result;

pub mod web_search;

pub use web_search::WebSearch;

/// Cloneable tool set for concurrent fanout arms.
///
/// Each arm builds a short-lived [`ToolRegistry`] that borrows these `Arc`s.
#[derive(Clone, Default)]
pub(crate) struct SharedTools {
    tools: Arc<[Arc<dyn Tool>]>,
}

impl std::fmt::Debug for SharedTools {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedTools")
            .field(
                "ids",
                &self.tools.iter().map(|tool| tool.id()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl SharedTools {
    /// Builds a shared set from caller-owned tool arcs.
    #[must_use]
    pub(crate) fn new(tools: &[Arc<dyn Tool>]) -> Self {
        Self {
            tools: Arc::from(tools.to_vec()),
        }
    }

    /// Borrowing registry over the shared arcs.
    #[must_use]
    pub(crate) fn registry(&self) -> ToolRegistry<'_> {
        ToolRegistry::new(self.tools.iter().map(AsRef::as_ref))
    }
}

/// The stable identity of a live tool.
///
/// Identity is structural over the server and tool name. The wire name used
/// in a model request is deliberately not identity: later capability binding
/// can advertise a selected tool under a prompt-local alias without changing
/// the live tool it dispatches.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ToolId {
    server: String,
    name: String,
}

impl ToolId {
    /// Builds an identity from its server and stable tool name.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolId;
    ///
    /// let id = ToolId::new("promptforge", "web_fetch");
    /// assert_eq!(id.server(), "promptforge");
    /// assert_eq!(id.name(), "web_fetch");
    /// ```
    #[must_use]
    pub fn new(server: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            name: name.into(),
        }
    }

    /// Returns the server that owns this identity namespace.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolId;
    ///
    /// assert_eq!(ToolId::new("promptforge", "web_fetch").server(), "promptforge");
    /// ```
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Returns the stable name within the publisher's namespace.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolId;
    ///
    /// assert_eq!(ToolId::new("promptforge", "web_fetch").name(), "web_fetch");
    /// ```
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A tool the executor can dispatch during a model's tool-call loop.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Returns the tool's stable live identity.
    fn id(&self) -> ToolId;

    /// Returns the concrete name used by the current model transport.
    ///
    /// This is not the tool's identity. It may later be replaced by a
    /// prompt-local alias when the tool is advertised to a model.
    fn wire_name(&self) -> &str;

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

/// An ordered collection of callable live tools.
///
/// The registry preserves every entry, including repeated identities. A later
/// binding phase owns collision validation; this type only provides faithful
/// iteration and identity-based lookup.
pub struct ToolRegistry<'a> {
    tools: Vec<&'a dyn Tool>,
}

impl std::fmt::Debug for ToolRegistry<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolRegistry")
            .field(
                "ids",
                &self.tools.iter().map(|tool| tool.id()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl<'a> ToolRegistry<'a> {
    /// Builds a registry in the order the live tools are supplied.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolRegistry;
    ///
    /// let registry = ToolRegistry::new(std::iter::empty());
    /// assert!(registry.is_empty());
    /// ```
    #[must_use]
    pub fn new(tools: impl IntoIterator<Item = &'a dyn Tool>) -> Self {
        Self {
            tools: tools.into_iter().collect(),
        }
    }

    /// Returns the number of live registry entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolRegistry;
    ///
    /// assert_eq!(ToolRegistry::new(std::iter::empty()).len(), 0);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Returns whether the registry has no live entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolRegistry;
    ///
    /// assert!(ToolRegistry::new(std::iter::empty()).is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Returns the live entries in registry order.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolRegistry;
    ///
    /// assert!(ToolRegistry::new(std::iter::empty()).tools().is_empty());
    /// ```
    #[must_use]
    pub fn tools(&self) -> &[&'a dyn Tool] {
        &self.tools
    }

    /// Returns the first live tool with `id`.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::{ToolId, ToolRegistry};
    ///
    /// let registry = ToolRegistry::new(std::iter::empty());
    /// assert!(registry.get(&ToolId::new("promptforge", "missing")).is_none());
    /// ```
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&'a dyn Tool> {
        self.tools.iter().copied().find(|tool| tool.id() == *id)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Tool, ToolId, ToolRegistry, WebSearch};
    use crate::Result;

    struct FixtureTool;

    #[async_trait::async_trait]
    impl Tool for FixtureTool {
        fn id(&self) -> ToolId {
            ToolId::new("fixtures", "inspect")
        }

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn wire_name(&self) -> &str {
            "inspect_wire"
        }

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn description(&self) -> &str {
            "Inspect a fixture."
        }

        fn parameters_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            })
        }

        async fn call(&self, _args: Value) -> Result<String> {
            Ok(String::new())
        }
    }

    struct RegistryFixtureTool {
        id_name: &'static str,
        wire_name: &'static str,
    }

    #[async_trait::async_trait]
    impl Tool for RegistryFixtureTool {
        fn id(&self) -> ToolId {
            ToolId::new("fixtures", self.id_name)
        }

        fn wire_name(&self) -> &str {
            self.wire_name
        }

        fn description(&self) -> &str {
            self.wire_name
        }

        fn parameters_schema(&self) -> Value {
            json!({"type": "object"})
        }

        async fn call(&self, _args: Value) -> Result<String> {
            Ok(String::new())
        }
    }

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

    #[test]
    fn descriptor_surface_preserves_identity_description_and_schema() {
        let tool = FixtureTool;

        assert_eq!(tool.id(), ToolId::new("fixtures", "inspect"));
        assert_eq!(tool.wire_name(), "inspect_wire");
        assert_eq!(tool.description(), "Inspect a fixture.");
        assert_eq!(
            tool.parameters_schema(),
            json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            })
        );
    }

    #[test]
    fn registry_lookup_uses_stable_identity_not_wire_name() {
        let tool = FixtureTool;
        let registry = ToolRegistry::new([&tool as &dyn Tool]);

        let found = registry
            .get(&ToolId::new("fixtures", "inspect"))
            .expect("the stable identity should resolve");
        assert_eq!(found.wire_name(), "inspect_wire");
        assert!(
            registry
                .get(&ToolId::new("fixtures", "inspect_wire"))
                .is_none(),
            "the transport name must not become identity"
        );
    }

    #[test]
    fn registry_preserves_order_duplicates_and_first_match_lookup() {
        let first = RegistryFixtureTool {
            id_name: "inspect",
            wire_name: "first_inspect",
        };
        let middle = RegistryFixtureTool {
            id_name: "summarize",
            wire_name: "summarize",
        };
        let repeated = RegistryFixtureTool {
            id_name: "inspect",
            wire_name: "second_inspect",
        };
        let registry = ToolRegistry::new([
            &first as &dyn Tool,
            &middle as &dyn Tool,
            &repeated as &dyn Tool,
        ]);

        assert_eq!(
            registry
                .tools()
                .iter()
                .map(|tool| tool.wire_name())
                .collect::<Vec<_>>(),
            ["first_inspect", "summarize", "second_inspect"]
        );
        assert_eq!(registry.len(), 3, "repeated identities must be retained");
        assert_eq!(
            registry
                .get(&ToolId::new("fixtures", "inspect"))
                .expect("the repeated identity should resolve")
                .wire_name(),
            "first_inspect",
            "lookup must return the first matching entry"
        );
    }
}
