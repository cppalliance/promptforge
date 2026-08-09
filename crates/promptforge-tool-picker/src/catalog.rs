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
//! name with its underscores opened out, its description, and the names of its
//! top-level parameters, joined in a fixed shape. That derivation is
//! deterministic - the same descriptor always yields the same string - because
//! ranking is only reproducible if the text behind each vector is, and the
//! shape is fixed because the crate's calibrated thresholds were measured
//! against text in exactly that shape.

use serde_json::Value;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// The stable identity of a tool: the server it lives on and its name there.
///
/// Two descriptors denote the same tool exactly when their identities compare
/// equal, and equality is structural over the pair - the parts are never
/// concatenated to compare or hash them. A tool name is only unique within its
/// server, so the server is part of the identity, not context around it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
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
}

/// The MCP behavioural hints a tool may advertise about itself.
///
/// Every hint is optional and every one is advisory: a catalog producer that
/// knows nothing about a tool's behaviour leaves them all `None`, which is the
/// default. The engine uses them only to break a tie between candidates that
/// are otherwise indistinguishable, so an absent hint never changes a ranking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub struct ToolAnnotations {
    /// The tool does not modify its environment.
    #[cfg_attr(
        feature = "serde",
        serde(
            default,
            rename = "readOnlyHint",
            skip_serializing_if = "Option::is_none"
        )
    )]
    read_only: Option<bool>,
    /// The tool may perform destructive updates.
    #[cfg_attr(
        feature = "serde",
        serde(
            default,
            rename = "destructiveHint",
            skip_serializing_if = "Option::is_none"
        )
    )]
    destructive: Option<bool>,
    /// Repeating the call with the same arguments has no additional effect.
    #[cfg_attr(
        feature = "serde",
        serde(
            default,
            rename = "idempotentHint",
            skip_serializing_if = "Option::is_none"
        )
    )]
    idempotent: Option<bool>,
}

impl ToolAnnotations {
    /// Builds annotations with every hint absent.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the read-only hint.
    #[must_use]
    pub fn read_only(&self) -> Option<bool> {
        self.read_only
    }

    /// Returns the destructive hint.
    #[must_use]
    pub fn destructive(&self) -> Option<bool> {
        self.destructive
    }

    /// Returns the idempotent hint.
    #[must_use]
    pub fn idempotent(&self) -> Option<bool> {
        self.idempotent
    }

    /// Returns these annotations with a read-only hint.
    #[must_use]
    pub fn with_read_only(mut self, value: bool) -> Self {
        self.read_only = Some(value);
        self
    }

    /// Returns these annotations with a destructive hint.
    #[must_use]
    pub fn with_destructive(mut self, value: bool) -> Self {
        self.destructive = Some(value);
        self
    }

    /// Returns these annotations with an idempotent hint.
    #[must_use]
    pub fn with_idempotent(mut self, value: bool) -> Self {
        self.idempotent = Some(value);
        self
    }
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
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub struct ToolDescriptor {
    /// The tool's stable identity.
    #[cfg_attr(feature = "serde", serde(flatten))]
    id: ToolId,
    /// Prose describing what the tool does, as its author wrote it.
    description: String,
    /// The tool's JSON Schema for its arguments.
    ///
    /// Only the top-level `properties` keys are read, and only to enrich the
    /// text that gets embedded; the schema is never validated or executed. Any
    /// JSON value is accepted, including a non-object one.
    #[cfg_attr(feature = "serde", serde(default, alias = "inputSchema"))]
    input_schema: Value,
    /// Optional behavioural hints, used only as a tie-break.
    #[cfg_attr(feature = "serde", serde(default))]
    annotations: ToolAnnotations,
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

    /// Returns the tool identity.
    #[must_use]
    pub fn id(&self) -> &ToolId {
        &self.id
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

    /// Returns the description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the input schema.
    #[must_use]
    pub fn input_schema(&self) -> &Value {
        &self.input_schema
    }

    /// Returns the behavioral annotations.
    #[must_use]
    pub fn annotations(&self) -> ToolAnnotations {
        self.annotations
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
    /// happened to arrive in. A JSON object is an unordered set of members, and
    /// the ordering the keys had in the source text is not recoverable once the
    /// document is parsed, so sorting is the one ordering that can be produced
    /// identically every time; the result does not depend on how the JSON was
    /// parsed or which map the parser used.
    #[must_use]
    pub(crate) fn parameter_names(&self) -> Vec<&str> {
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
    /// The text is built from up to three parts, in this order:
    ///
    /// 1. the tool's name with every `_` replaced by a single space, so
    ///    `read_file` reads as `read file`;
    /// 2. the description exactly as its author wrote it;
    /// 3. the literal `parameters: ` followed by the parameter names joined
    ///    with `, `, present only when there is at least one parameter.
    ///
    /// Empty parts are dropped, and the parts that remain are joined with
    /// `. ` - a period and a space. A description that already ends in a
    /// period therefore yields a doubled period before the parameter part;
    /// that is intended and is not collapsed. Parameters are included because
    /// a name and a one-line description often under-describe a tool, while
    /// its argument names say concretely what it operates on, and the name is
    /// de-underscored because the embedding model reads a snake_case
    /// identifier as one opaque token rather than as its words.
    ///
    /// The exact shape matters beyond taste. The thresholds in
    /// [`Config`](crate::Config) - the similarity floor above all - are
    /// calibrated numbers, and they are only meaningful against vectors of
    /// text in this shape. Changing the punctuation, the ordering, or the
    /// `parameters: ` prefix moves every similarity score and silently
    /// invalidates those defaults.
    ///
    /// The result is a pure function of the descriptor: parameter names are
    /// ordered by [`ToolDescriptor::parameter_names`], so repeated calls, and
    /// separate runs over the same catalog, produce byte-identical text and
    /// therefore identical vectors.
    #[must_use]
    pub(crate) fn enriched_text(&self) -> String {
        let parameters = self.parameter_names();
        let mut text = self.name().replace('_', " ");
        if !self.description.is_empty() {
            if !text.is_empty() {
                text.push_str(". ");
            }
            text.push_str(&self.description);
        }
        if !parameters.is_empty() {
            if !text.is_empty() {
                text.push_str(". ");
            }
            text.push_str("parameters: ");
            for (index, parameter) in parameters.iter().enumerate() {
                if index != 0 {
                    text.push_str(", ");
                }
                text.push_str(parameter);
            }
        }
        text
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(transparent))]
#[non_exhaustive]
pub struct Catalog {
    tools: Vec<ToolDescriptor>,
}

impl Catalog {
    /// Builds a catalog from descriptors, preserving their order.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_tool_picker::{Catalog, ToolDescriptor, ToolId};
    /// use serde_json::json;
    ///
    /// let catalog = Catalog::new(vec![
    ///     ToolDescriptor::new(
    ///         ToolId::new("files", "read_file"),
    ///         "Read a file from disk",
    ///         json!({"properties": {"path": {"type": "string"}}}),
    ///     ),
    ///     ToolDescriptor::new(
    ///         ToolId::new("net", "fetch_url"),
    ///         "Fetch a web page over HTTP",
    ///         json!({"properties": {"url": {"type": "string"}}}),
    ///     ),
    /// ]);
    ///
    /// assert_eq!(catalog.len(), 2);
    /// // The order given is the order kept, and it is the order every
    /// // per-tool accessor on an engine is indexed by.
    /// assert_eq!(catalog.tools()[0].name(), "read_file");
    /// ```
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

    /// Iterates the descriptors in the order they were given.
    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter(&self) -> CatalogIter<'_> {
        self.tools.iter()
    }

    /// Iterates mutably over descriptors in catalog order.
    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter_mut(&mut self) -> CatalogIterMut<'_> {
        self.tools.iter_mut()
    }

    /// The first descriptor with the given identity, if the catalog has one.
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&ToolDescriptor> {
        self.tools.iter().find(|tool| tool.id() == id)
    }

    pub(crate) fn as_slice(&self) -> &[ToolDescriptor] {
        &self.tools
    }
}

/// A shared catalog iterator.
pub type CatalogIter<'a> = std::slice::Iter<'a, ToolDescriptor>;
/// A mutable catalog iterator.
pub type CatalogIterMut<'a> = std::slice::IterMut<'a, ToolDescriptor>;
/// An owning catalog iterator.
pub type CatalogIntoIter = std::vec::IntoIter<ToolDescriptor>;

impl<'a> IntoIterator for &'a Catalog {
    type Item = &'a ToolDescriptor;
    type IntoIter = CatalogIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for Catalog {
    type Item = ToolDescriptor;
    type IntoIter = CatalogIntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.tools.into_iter()
    }
}

impl<'a> IntoIterator for &'a mut Catalog {
    type Item = &'a mut ToolDescriptor;
    type IntoIter = CatalogIterMut<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl From<Vec<ToolDescriptor>> for Catalog {
    fn from(tools: Vec<ToolDescriptor>) -> Self {
        Self::new(tools)
    }
}

impl FromIterator<ToolDescriptor> for Catalog {
    fn from_iter<I: IntoIterator<Item = ToolDescriptor>>(iter: I) -> Self {
        Self::new(iter.into_iter().collect())
    }
}

#[cfg(all(test, not(test)))]
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
            "read file. Read a file from disk. parameters: encoding, offset, path"
        );
    }

    #[test]
    fn enriched_text_without_parameters_is_name_and_description() {
        for schema in [json!({}), json!({"type": "object", "properties": {}})] {
            let tool = descriptor(schema);
            assert!(tool.parameter_names().is_empty());
            assert_eq!(tool.enriched_text(), "read file. Read a file from disk");
        }
    }

    #[test]
    fn empty_description_does_not_double_the_separator() {
        let tool = ToolDescriptor::new(
            ToolId::new("files", "read_file"),
            "",
            json!({"properties": {"path": {"type": "string"}}}),
        );
        assert_eq!(tool.enriched_text(), "read file. parameters: path");
    }

    #[test]
    fn a_description_ending_in_a_period_keeps_the_doubled_period() {
        let tool = ToolDescriptor::new(
            ToolId::new("files", "read_file"),
            "Read a file from disk.",
            json!({"properties": {"path": {}, "encoding": {}}}),
        );
        assert_eq!(
            tool.enriched_text(),
            "read file. Read a file from disk.. parameters: encoding, path"
        );
    }

    #[test]
    fn a_tool_without_parameters_omits_the_parameters_part() {
        let tool = ToolDescriptor::new(ToolId::new("meta", "list_tools"), "List tools.", json!({}));
        assert_eq!(tool.enriched_text(), "list tools. List tools.");
    }

    #[test]
    fn non_object_schema_yields_no_parameters() {
        for schema in [Value::Null, json!(true), json!("string"), json!([1, 2, 3])] {
            let tool = descriptor(schema);
            assert!(
                tool.parameter_names().is_empty(),
                "a non-object schema must not produce parameter names"
            );
            assert_eq!(tool.enriched_text(), "read file. Read a file from disk");
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
            "read file. Read a file from disk. parameters: path"
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

#[cfg(test)]
mod contract_tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn enriched_text_is_byte_exact_and_structural_ids_do_not_collide() {
        let tool = ToolDescriptor::new(
            ToolId::new("a", "read_file"),
            "Read.",
            json!({"properties": {"z": {}, "a": {}}}),
        );
        assert_eq!(tool.enriched_text(), "read file. Read.. parameters: a, z");
        assert_ne!(ToolId::new("a", "b\u{1f}c"), ToolId::new("a\u{1f}b", "c"));
    }

    #[test]
    fn annotations_and_catalog_iteration_preserve_values_and_order() {
        let annotations = ToolAnnotations::new()
            .with_read_only(false)
            .with_destructive(true)
            .with_idempotent(false);
        assert_eq!(annotations.read_only(), Some(false));
        assert_eq!(annotations.destructive(), Some(true));
        assert_eq!(annotations.idempotent(), Some(false));

        let first = ToolDescriptor::new(ToolId::new("s", "first"), "", json!({}));
        let second = ToolDescriptor::new(ToolId::new("s", "second"), "", json!({}));
        let mut catalog = Catalog::from(vec![first.clone(), second.clone(), first.clone()]);
        assert_eq!(
            catalog.iter().map(ToolDescriptor::name).collect::<Vec<_>>(),
            ["first", "second", "first"]
        );
        assert_eq!(catalog.iter_mut().count(), 3);
        assert_eq!(catalog.get(first.id()), Some(&first));
        assert_eq!(catalog.into_iter().count(), 3);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_wire_shape_and_alias_are_preserved() {
        let descriptor: ToolDescriptor = serde_json::from_value(json!({
            "server": "files",
            "name": "read",
            "description": "Read",
            "inputSchema": {"type": "object"},
            "annotations": {"readOnlyHint": true}
        }))
        .unwrap();
        assert_eq!(descriptor.id(), &ToolId::new("files", "read"));
        assert_eq!(descriptor.annotations().read_only(), Some(true));
    }
}
