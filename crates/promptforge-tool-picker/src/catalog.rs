//! The tool catalog: immutable identity, descriptors, and the ordered set.
//!
//! A catalog is the set of tool descriptors the engine may choose from, each
//! carrying the identity and prose that the embedding is derived from.
//!
//! Identity is the `(server, name)` pair, modelled as [`ToolId`]. The pair is
//! kept structural rather than folded into one string, so a server or tool name
//! containing any delimiter stays unambiguous. There is no concatenated key.
//!
//! The prose an embedding sees is the descriptor's internal enriched text: the
//! tool name with its underscores opened out, its description, and the names of
//! its top-level parameters, joined in a fixed shape. That derivation is a
//! private, calibrated internal encoding, not a caller operation.

use serde_json::Value;

/// The stable identity of a tool: the server it lives on and its name there.
///
/// Two descriptors denote the same tool exactly when their identities compare
/// equal, and equality is structural over the pair - the parts are never
/// concatenated. A tool name is only unique within its server, so the server is
/// part of the identity, not context around it.
///
/// # Examples
///
/// ```
/// use promptforge_tool_picker::ToolId;
///
/// let id = ToolId::new("files", "read_file");
/// assert_eq!(id.server(), "files");
/// assert_eq!(id.name(), "read_file");
/// // A delimiter inside either part never collides two identities.
/// assert_ne!(ToolId::new("a/b", "c"), ToolId::new("a", "b/c"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

    /// Returns the server the tool is served from.
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Returns the tool's name within its server.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// The MCP behavioural hints a tool may advertise about itself.
///
/// Every hint is optional and advisory: a catalog producer that knows nothing
/// about a tool's behaviour leaves them all absent, which is the default. The
/// engine uses them only to break a tie between otherwise indistinguishable
/// candidates, so an absent hint never changes a ranking.
///
/// # Examples
///
/// ```
/// use promptforge_tool_picker::ToolAnnotations;
///
/// let hints = ToolAnnotations::new().with_read_only(true);
/// assert_eq!(hints.read_only(), Some(true));
/// assert_eq!(hints.destructive(), None);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    /// Builds annotations with no hints claimed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the read-only hint, if the tool claimed one.
    #[must_use]
    pub fn read_only(&self) -> Option<bool> {
        self.read_only
    }

    /// Returns the destructive hint, if the tool claimed one.
    #[must_use]
    pub fn destructive(&self) -> Option<bool> {
        self.destructive
    }

    /// Returns the idempotent hint, if the tool claimed one.
    #[must_use]
    pub fn idempotent(&self) -> Option<bool> {
        self.idempotent
    }

    /// Returns these annotations with the read-only hint set.
    #[must_use]
    pub fn with_read_only(mut self, value: bool) -> Self {
        self.read_only = Some(value);
        self
    }

    /// Returns these annotations with the destructive hint set.
    #[must_use]
    pub fn with_destructive(mut self, value: bool) -> Self {
        self.destructive = Some(value);
        self
    }

    /// Returns these annotations with the idempotent hint set.
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
/// The JSON Schema value is retained intentionally: an arbitrary schema is
/// catalog input data, not an embedding-backend detail. The crate never
/// validates or executes it.
///
/// In JSON the identity is flat, so a catalog entry reads as
/// `{"server": ..., "name": ..., "description": ..., "input_schema": ...}`. The
/// schema field also accepts its MCP spelling, `inputSchema`.
///
/// # Examples
///
/// ```
/// use promptforge_tool_picker::{ToolDescriptor, ToolId};
/// use serde_json::json;
///
/// let tool = ToolDescriptor::new(
///     ToolId::new("files", "read_file"),
///     "Read a file from disk",
///     json!({"properties": {"path": {"type": "string"}}}),
/// );
/// assert_eq!(tool.name(), "read_file");
/// assert_eq!(tool.description(), "Read a file from disk");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ToolDescriptor {
    /// The tool's stable identity.
    #[cfg_attr(feature = "serde", serde(flatten))]
    id: ToolId,
    /// Prose describing what the tool does, as its author wrote it.
    description: String,
    /// The tool's JSON Schema for its arguments.
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

    /// Returns the tool's stable identity.
    #[must_use]
    pub fn id(&self) -> &ToolId {
        &self.id
    }

    /// Returns the server the tool is served from.
    #[must_use]
    pub fn server(&self) -> &str {
        self.id.server()
    }

    /// Returns the tool's name within its server.
    #[must_use]
    pub fn name(&self) -> &str {
        self.id.name()
    }

    /// Returns the prose describing what the tool does.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the tool's JSON Schema for its arguments.
    #[must_use]
    pub fn input_schema(&self) -> &Value {
        &self.input_schema
    }

    /// Returns the tool's behavioural hints.
    #[must_use]
    pub fn annotations(&self) -> ToolAnnotations {
        self.annotations
    }

    /// The names of the tool's top-level parameters, in ascending order.
    ///
    /// The names are the keys of the `properties` object at the root of the
    /// input schema. A schema that is not an object, has no `properties`, or
    /// whose `properties` is not an object yields no names.
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

    /// The calibrated text the engine embeds for this tool.
    ///
    /// Built from the de-underscored name, the description, and a
    /// `parameters: ` tail, joined with `. `. The exact shape is calibration
    /// sensitive: the configured thresholds are only meaningful against text in
    /// this shape.
    pub(crate) fn enriched_text(&self) -> String {
        let parameters = self.parameter_names();
        let mut text = String::with_capacity(
            self.name().len() + self.description.len() + parameters.len() * 8,
        );
        let mut wrote = false;
        let name = self.name().replace('_', " ");
        if !name.is_empty() {
            text.push_str(&name);
            wrote = true;
        }
        if !self.description.is_empty() {
            if wrote {
                text.push_str(". ");
            }
            text.push_str(&self.description);
            wrote = true;
        }
        if !parameters.is_empty() {
            if wrote {
                text.push_str(". ");
            }
            text.push_str("parameters: ");
            for (index, parameter) in parameters.iter().enumerate() {
                if index > 0 {
                    text.push_str(", ");
                }
                text.push_str(parameter);
            }
        }
        text
    }
}

/// A borrowed iterator over a catalog's descriptors.
pub type CatalogIter<'a> = std::slice::Iter<'a, ToolDescriptor>;
/// A mutable iterator over a catalog's descriptors.
pub type CatalogIterMut<'a> = std::slice::IterMut<'a, ToolDescriptor>;
/// An owning iterator over a catalog's descriptors.
pub type CatalogIntoIter = std::vec::IntoIter<ToolDescriptor>;

/// The set of tools the engine may choose from.
///
/// This is the crate's sole input contract. A catalog is an ordered collection
/// of descriptors; order is preserved, and duplicate identities are accepted
/// because two tools describing the same capability are a result the engine
/// reports, not an input it refuses.
///
/// In JSON a catalog is simply an array of descriptors.
///
/// # Examples
///
/// ```
/// use promptforge_tool_picker::{Catalog, ToolDescriptor, ToolId};
/// use serde_json::json;
///
/// let catalog = Catalog::new(vec![ToolDescriptor::new(
///     ToolId::new("files", "read_file"),
///     "Read a file from disk",
///     json!({"properties": {"path": {"type": "string"}}}),
/// )]);
/// assert_eq!(catalog.len(), 1);
/// assert_eq!(catalog.iter().next().map(ToolDescriptor::name), Some("read_file"));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
#[non_exhaustive]
pub struct Catalog {
    /// The descriptors, in the order they were given.
    tools: Vec<ToolDescriptor>,
}

impl Catalog {
    /// Builds a catalog from descriptors, preserving their order.
    #[must_use]
    pub fn new(tools: Vec<ToolDescriptor>) -> Self {
        Self { tools }
    }

    /// Returns the number of tools in the catalog.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Returns whether the catalog holds no tools.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Iterates the descriptors in the order they were given.
    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter(&self) -> CatalogIter<'_> {
        self.tools.iter()
    }

    /// Iterates the descriptors mutably, in the order they were given.
    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter_mut(&mut self) -> CatalogIterMut<'_> {
        self.tools.iter_mut()
    }

    /// Returns the first descriptor with the given identity, if any.
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&ToolDescriptor> {
        self.tools.iter().find(|tool| tool.id() == id)
    }

    /// The descriptors as a slice, for crate-internal indexing.
    pub(crate) fn as_slice(&self) -> &[ToolDescriptor] {
        &self.tools
    }
}

impl<'a> IntoIterator for &'a Catalog {
    type Item = &'a ToolDescriptor;
    type IntoIter = CatalogIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.tools.iter()
    }
}

impl<'a> IntoIterator for &'a mut Catalog {
    type Item = &'a mut ToolDescriptor;
    type IntoIter = CatalogIterMut<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.tools.iter_mut()
    }
}

impl IntoIterator for Catalog {
    type Item = ToolDescriptor;
    type IntoIter = CatalogIntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.tools.into_iter()
    }
}

impl FromIterator<ToolDescriptor> for Catalog {
    fn from_iter<I: IntoIterator<Item = ToolDescriptor>>(iter: I) -> Self {
        Self::new(iter.into_iter().collect())
    }
}

impl From<Vec<ToolDescriptor>> for Catalog {
    fn from(tools: Vec<ToolDescriptor>) -> Self {
        Self::new(tools)
    }
}

#[cfg(test)]
mod tests {
    use super::{Catalog, ToolAnnotations, ToolDescriptor, ToolId};
    use serde_json::{Value, json};

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
            assert!(tool.parameter_names().is_empty());
            assert_eq!(tool.enriched_text(), "read file. Read a file from disk");
        }
    }

    #[test]
    fn enriched_text_is_stable_across_key_order() {
        let one = descriptor(json!({"properties": {"zeta": {}, "alpha": {}, "mid": {}}}));
        let other = descriptor(json!({"properties": {"mid": {}, "alpha": {}, "zeta": {}}}));
        assert_eq!(one.enriched_text(), other.enriched_text());
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
    fn identities_with_a_delimiter_do_not_collide() {
        assert_ne!(
            ToolId::new("a\u{1f}b", "c"),
            ToolId::new("a", "b\u{1f}c"),
            "structural identity keeps a delimiter-bearing pair distinct"
        );
    }

    #[test]
    fn annotation_builders_and_accessors_cover_absent_true_and_false() {
        let hints = ToolAnnotations::new()
            .with_read_only(true)
            .with_destructive(false);
        assert_eq!(hints.read_only(), Some(true));
        assert_eq!(hints.destructive(), Some(false));
        assert_eq!(hints.idempotent(), None);
    }

    #[test]
    fn catalog_reports_size_iterates_and_looks_up_first_match() {
        let empty = Catalog::default();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let first = descriptor(json!({}));
        let second = ToolDescriptor::new(ToolId::new("net", "fetch"), "Fetch a URL", json!({}));
        let catalog = Catalog::new(vec![first.clone(), second.clone()]);
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog.iter().collect::<Vec<_>>(), vec![&first, &second]);
        assert_eq!(catalog.get(second.id()), Some(&second));
        assert_eq!(catalog.get(&ToolId::new("net", "missing")), None);
    }

    #[test]
    fn catalog_iterates_mutably_and_owns_from_vec_and_from_iter() {
        let mut catalog = Catalog::from(vec![
            ToolDescriptor::new(ToolId::new("a", "one"), "one", json!({})),
            ToolDescriptor::new(ToolId::new("b", "two"), "two", json!({})),
        ]);
        for tool in &mut catalog {
            *tool = tool
                .clone()
                .with_annotations(ToolAnnotations::new().with_read_only(true));
        }
        assert!(
            catalog
                .iter()
                .all(|tool| tool.annotations().read_only() == Some(true))
        );

        let collected: Catalog = catalog.clone().into_iter().collect();
        assert_eq!(collected, catalog);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn descriptor_deserializes_from_a_flat_mcp_shaped_object() {
        let parsed: ToolDescriptor = serde_json::from_value(json!({
            "server": "files",
            "name": "read_file",
            "description": "Read a file from disk",
            "inputSchema": {"properties": {"path": {"type": "string"}}},
            "annotations": {"readOnlyHint": true}
        }))
        .expect("flat MCP descriptor deserializes");
        assert_eq!(parsed.id(), &ToolId::new("files", "read_file"));
        assert_eq!(parsed.annotations().read_only(), Some(true));
        assert_eq!(parsed.annotations().destructive(), None);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn absent_optional_fields_default() {
        let parsed: ToolDescriptor = serde_json::from_value(json!({
            "server": "files",
            "name": "read_file",
            "description": "Read a file from disk"
        }))
        .expect("descriptor with absent optionals deserializes");
        assert_eq!(parsed.input_schema(), &Value::Null);
        assert_eq!(parsed.annotations(), ToolAnnotations::default());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn catalog_round_trips_as_an_array() {
        let catalog = Catalog::new(vec![
            descriptor(json!({"properties": {"path": {"type": "string"}}}))
                .with_annotations(ToolAnnotations::new().with_read_only(true)),
            ToolDescriptor::new(ToolId::new("net", "fetch"), "Fetch a URL", json!({})),
        ]);
        let text = serde_json::to_string(&catalog).expect("serialize");
        assert!(text.starts_with('['), "a catalog serializes as an array");
        let parsed: Catalog = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(parsed, catalog);
    }
}
