//! The tool catalog: the sole input contract of the engine.
//!
//! A catalog is the set of tool descriptors the engine may choose from, each
//! carrying the identity and prose that the embedding is derived from.
//!
//! Identity is the `(server, name)` pair, modelled as [`ToolId`]. The pair is
//! kept as a pair rather than folded into one string, so a server or tool name
//! containing any delimiter a caller might have chosen stays unambiguous.
//!
//! The prose an embedding sees is [`ToolDescriptor::enriched_text`]: the tool
//! name, its description, and the names of its top-level parameters. That
//! derivation is deterministic - the same descriptor always yields the same
//! string - because ranking is only reproducible if the text behind each vector
//! is.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The delimiter [`ToolId::qualified_key`] puts between server and tool name.
///
/// ASCII unit separator (`U+001F`). It is a C0 control character, so it cannot
/// occur in a server or tool name that any protocol or filesystem would accept;
/// that is the whole reason it is used, and it is what makes the qualified key
/// unambiguous. A printable delimiter such as `/`, `.`, or `:` can legally
/// appear inside either part and would let two distinct identities collide.
pub const QUALIFIED_KEY_SEPARATOR: char = '\u{001f}';

/// The stable identity of a tool: the server it lives on and its name there.
///
/// Two descriptors denote the same tool exactly when their identities compare
/// equal, and equality is structural over the pair - the parts are never
/// concatenated to compare or hash them. A tool name is only unique within its
/// server, so the server is part of the identity, not context around it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ToolId {
    /// The server the tool is served from.
    server: String,
    /// The tool's name within that server.
    name: String,
}

impl ToolId {
    /// Builds an identity from a server and a tool name.
    #[must_use]
    pub fn new(server: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            name: name.into(),
        }
    }

    /// The server the tool is served from.
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    /// The tool's name within its server.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// A single-string form of the identity, for keying or logging.
    ///
    /// The two parts are joined by [`QUALIFIED_KEY_SEPARATOR`], which cannot
    /// occur inside either part, so the key round-trips: distinct identities
    /// always produce distinct keys and the key splits back into exactly the
    /// pair it was built from. It is a machine key, not a label for a human -
    /// the separator does not print.
    #[must_use]
    pub fn qualified_key(&self) -> String {
        format!("{}{QUALIFIED_KEY_SEPARATOR}{}", self.server, self.name)
    }
}

/// The MCP behavioural hints a tool may advertise about itself.
///
/// Every hint is optional and every one is advisory: a catalog producer that
/// knows nothing about a tool's behaviour leaves them all `None`, which is the
/// default. The engine uses them only to break a tie between candidates that
/// are otherwise indistinguishable, so an absent hint never changes a ranking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolAnnotations {
    /// The tool does not modify its environment.
    #[serde(
        default,
        rename = "readOnlyHint",
        skip_serializing_if = "Option::is_none"
    )]
    pub read_only: Option<bool>,
    /// The tool may perform destructive updates.
    #[serde(
        default,
        rename = "destructiveHint",
        skip_serializing_if = "Option::is_none"
    )]
    pub destructive: Option<bool>,
    /// Repeating the call with the same arguments has no additional effect.
    #[serde(
        default,
        rename = "idempotentHint",
        skip_serializing_if = "Option::is_none"
    )]
    pub idempotent: Option<bool>,
}

/// One tool the engine may resolve a need to.
///
/// A descriptor is abstract: it says what a tool is called, what it claims to
/// do, and what it takes, but carries nothing that could invoke it. Mapping a
/// resolved descriptor onto something callable is the caller's job.
///
/// In JSON the identity is flat, so a catalog entry reads as
/// `{"server": ..., "name": ..., "description": ..., "input_schema": ...}`.
/// The schema field also accepts its MCP spelling, `inputSchema`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    /// The tool's stable identity.
    #[serde(flatten)]
    pub id: ToolId,
    /// Prose describing what the tool does, as its author wrote it.
    pub description: String,
    /// The tool's JSON Schema for its arguments.
    ///
    /// Only the top-level `properties` keys are read, and only to enrich the
    /// text that gets embedded; the schema is never validated or executed. Any
    /// JSON value is accepted, including a non-object one.
    #[serde(default, alias = "inputSchema")]
    pub input_schema: Value,
    /// Optional behavioural hints, used only as a tie-break.
    #[serde(default)]
    pub annotations: ToolAnnotations,
}

impl ToolDescriptor {
    /// Builds a descriptor with no behavioural hints.
    #[must_use]
    pub fn new(id: ToolId, description: impl Into<String>, input_schema: Value) -> Self {
        Self {
            id,
            description: description.into(),
            input_schema,
            annotations: ToolAnnotations::default(),
        }
    }

    /// Returns this descriptor with the given behavioural hints attached.
    #[must_use]
    pub fn with_annotations(mut self, annotations: ToolAnnotations) -> Self {
        self.annotations = annotations;
        self
    }

    /// The server the tool is served from.
    #[must_use]
    pub fn server(&self) -> &str {
        self.id.server()
    }

    /// The tool's name within its server.
    #[must_use]
    pub fn name(&self) -> &str {
        self.id.name()
    }

    /// The names of the tool's top-level parameters, in ascending order.
    ///
    /// The names are the keys of the `properties` object at the root of
    /// [`ToolDescriptor::input_schema`]. A schema that is not an object, has no
    /// `properties`, or whose `properties` is not an object yields no names -
    /// a malformed or absent schema costs the tool its parameter names, never
    /// an error.
    ///
    /// The order is ascending by byte value rather than the order the keys
    /// happened to arrive in, so the result does not depend on how the JSON was
    /// parsed or which map the parser used.
    #[must_use]
    pub fn parameter_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .input_schema
            .as_object()
            .and_then(|schema| schema.get("properties"))
            .and_then(Value::as_object)
            .map(|properties| properties.keys().map(String::as_str).collect())
            .unwrap_or_default();
        names.sort_unstable();
        names
    }

    /// The text the engine embeds for this tool.
    ///
    /// It is the tool's name, then its description, then its parameter names,
    /// separated by single spaces. Parameters are included because a name and
    /// a one-line description often under-describe a tool, while its argument
    /// names say concretely what it operates on.
    ///
    /// The result is a pure function of the descriptor: parameter names are
    /// ordered by [`ToolDescriptor::parameter_names`], so repeated calls, and
    /// separate runs over the same catalog, produce byte-identical text and
    /// therefore identical vectors.
    #[must_use]
    pub fn enriched_text(&self) -> String {
        let parameters = self.parameter_names();
        let mut parts: Vec<&str> = Vec::with_capacity(2 + parameters.len());
        parts.push(self.name());
        if !self.description.is_empty() {
            parts.push(&self.description);
        }
        parts.extend(parameters);
        parts.join(" ")
    }
}

/// The set of tools the engine may choose from.
///
/// This is the crate's sole input contract. A catalog is an ordered collection
/// of descriptors; order is preserved as given so that a caller can reproduce
/// a ranking's tie-breaks, and duplicates are not rejected here - two tools
/// that describe the same capability are a result the engine reports, not an
/// input it refuses.
///
/// In JSON a catalog is simply an array of descriptors.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Catalog {
    tools: Vec<ToolDescriptor>,
}

impl Catalog {
    /// Builds a catalog from descriptors, preserving their order.
    #[must_use]
    pub fn new(tools: Vec<ToolDescriptor>) -> Self {
        Self { tools }
    }

    /// The number of tools in the catalog.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the catalog holds no tools.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// The descriptors, in the order they were given.
    #[must_use]
    pub fn tools(&self) -> &[ToolDescriptor] {
        &self.tools
    }

    /// Iterates the descriptors in the order they were given.
    pub fn iter(&self) -> std::slice::Iter<'_, ToolDescriptor> {
        self.tools.iter()
    }

    /// The first descriptor with the given identity, if the catalog has one.
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&ToolDescriptor> {
        self.tools.iter().find(|tool| &tool.id == id)
    }
}

impl<'a> IntoIterator for &'a Catalog {
    type Item = &'a ToolDescriptor;
    type IntoIter = std::slice::Iter<'a, ToolDescriptor>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for Catalog {
    type Item = ToolDescriptor;
    type IntoIter = std::vec::IntoIter<ToolDescriptor>;

    fn into_iter(self) -> Self::IntoIter {
        self.tools.into_iter()
    }
}

impl FromIterator<ToolDescriptor> for Catalog {
    fn from_iter<I: IntoIterator<Item = ToolDescriptor>>(iter: I) -> Self {
        Self::new(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{Catalog, QUALIFIED_KEY_SEPARATOR, ToolAnnotations, ToolDescriptor, ToolId};
    use serde_json::{Value, json};

    /// A descriptor with the given schema and fixed identity and prose.
    fn descriptor(schema: Value) -> ToolDescriptor {
        ToolDescriptor::new(
            ToolId::new("files", "read_file"),
            "Read a file from disk",
            schema,
        )
    }

    #[test]
    fn enriched_text_appends_sorted_parameter_names() {
        let tool = descriptor(json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "encoding": {"type": "string"},
                "offset": {"type": "integer"}
            }
        }));
        assert_eq!(
            tool.enriched_text(),
            "read_file Read a file from disk encoding offset path"
        );
    }

    #[test]
    fn enriched_text_without_parameters_is_name_and_description() {
        for schema in [json!({}), json!({"type": "object", "properties": {}})] {
            let tool = descriptor(schema);
            assert!(tool.parameter_names().is_empty());
            assert_eq!(tool.enriched_text(), "read_file Read a file from disk");
        }
    }

    #[test]
    fn empty_description_does_not_double_the_separator() {
        let tool = ToolDescriptor::new(
            ToolId::new("files", "read_file"),
            "",
            json!({"properties": {"path": {"type": "string"}}}),
        );
        assert_eq!(tool.enriched_text(), "read_file path");
    }

    #[test]
    fn non_object_schema_yields_no_parameters() {
        for schema in [Value::Null, json!(true), json!("string"), json!([1, 2, 3])] {
            let tool = descriptor(schema);
            assert!(
                tool.parameter_names().is_empty(),
                "a non-object schema must not produce parameter names"
            );
            assert_eq!(tool.enriched_text(), "read_file Read a file from disk");
        }
    }

    #[test]
    fn non_object_properties_yield_no_parameters() {
        let tool = descriptor(json!({"type": "object", "properties": ["path"]}));
        assert!(tool.parameter_names().is_empty());
    }

    #[test]
    fn enriched_text_is_stable_across_calls_and_key_order() {
        let one = descriptor(json!({
            "properties": {"zeta": {}, "alpha": {}, "mid": {}}
        }));
        let other = descriptor(json!({
            "properties": {"mid": {}, "alpha": {}, "zeta": {}}
        }));
        let text = one.enriched_text();
        for _ in 0..8 {
            assert_eq!(one.enriched_text(), text);
        }
        assert_eq!(
            other.enriched_text(),
            text,
            "key order in the schema must not change the embedded text"
        );
    }

    #[test]
    fn identity_is_the_server_and_name_pair() {
        let id = ToolId::new("files", "read_file");
        assert_eq!(id, ToolId::new("files", "read_file"));
        assert_ne!(id, ToolId::new("blobs", "read_file"));
        assert_ne!(id, ToolId::new("files", "write_file"));
        assert_eq!(id.server(), "files");
        assert_eq!(id.name(), "read_file");
    }

    #[test]
    fn qualified_key_separates_parts_unambiguously() {
        let id = ToolId::new("files", "read_file");
        let key = id.qualified_key();
        assert_eq!(
            key.split(QUALIFIED_KEY_SEPARATOR).collect::<Vec<_>>(),
            vec!["files", "read_file"]
        );
        // Parts holding a delimiter a caller might have picked still key apart.
        let left = ToolId::new("a/b", "c").qualified_key();
        let right = ToolId::new("a", "b/c").qualified_key();
        assert_ne!(left, right);
    }

    #[test]
    fn catalog_reports_size_and_iterates_in_order() {
        let empty = Catalog::default();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let first = descriptor(json!({}));
        let second = ToolDescriptor::new(ToolId::new("net", "fetch"), "Fetch a URL", json!({}));
        let catalog = Catalog::new(vec![first.clone(), second.clone()]);
        assert_eq!(catalog.len(), 2);
        assert!(!catalog.is_empty());
        assert_eq!(catalog.iter().collect::<Vec<_>>(), vec![&first, &second]);
        assert_eq!(catalog.get(&second.id), Some(&second));
        assert_eq!(catalog.get(&ToolId::new("net", "missing")), None);
        assert_eq!(catalog.tools().len(), 2);
    }

    #[test]
    fn catalog_round_trips_through_json() {
        let catalog = Catalog::new(vec![
            descriptor(json!({"properties": {"path": {"type": "string"}}})).with_annotations(
                ToolAnnotations {
                    read_only: Some(true),
                    ..ToolAnnotations::default()
                },
            ),
            ToolDescriptor::new(ToolId::new("net", "fetch"), "Fetch a URL", json!({})),
        ]);

        let text = serde_json::to_string(&catalog).unwrap();
        let parsed: Catalog = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, catalog);
    }

    #[test]
    fn descriptor_deserializes_from_a_flat_mcp_shaped_object() {
        let parsed: ToolDescriptor = serde_json::from_value(json!({
            "server": "files",
            "name": "read_file",
            "description": "Read a file from disk",
            "inputSchema": {"properties": {"path": {"type": "string"}}},
            "annotations": {"readOnlyHint": true}
        }))
        .unwrap();

        assert_eq!(parsed.id, ToolId::new("files", "read_file"));
        assert_eq!(parsed.server(), "files");
        assert_eq!(parsed.name(), "read_file");
        assert_eq!(parsed.annotations.read_only, Some(true));
        assert_eq!(parsed.annotations.destructive, None);
        assert_eq!(
            parsed.enriched_text(),
            "read_file Read a file from disk path"
        );
    }

    #[test]
    fn absent_optional_fields_default() {
        let parsed: ToolDescriptor = serde_json::from_value(json!({
            "server": "files",
            "name": "read_file",
            "description": "Read a file from disk"
        }))
        .unwrap();

        assert_eq!(parsed.input_schema, Value::Null);
        assert_eq!(parsed.annotations, ToolAnnotations::default());
    }
}
