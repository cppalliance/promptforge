//! The run's journaled bindings: [`ModelBindings`] and [`ToolBindings`].

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use promptforge_api_types::tools::Tool;
use promptforge_lua::Conflict;

use crate::model::{ModelDescriptor, ModelId};
use crate::tools::ToolId;

/// The run's model satisfaction: which concrete model each declared role
/// is bound to, and the descriptors of every model this run may use.
///
/// Written by the fill function at
/// [`prepare`](super::Environment::prepare); v1's fill is deliberately
/// trivial - every declared role binds to the context's current model.
/// The structure is general from day one (a table of models and a map of
/// roles) so multi-model satisfaction arrives as a smarter fill function,
/// never a structural change. Handles resolve label -> id -> descriptor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModelBindings {
    /// The decision, journaled: role label to the bound model's identity.
    roles: BTreeMap<String, ModelId>,
    /// What this run may use: identity to descriptor.
    models: BTreeMap<ModelId, ModelDescriptor>,
}

impl ModelBindings {
    /// Binds the role `label` to `model`, recording the descriptor under
    /// its identity. The fill function's only writer.
    pub(crate) fn bind(&mut self, label: &str, model: ModelDescriptor) {
        self.roles.insert(label.to_owned(), model.id().clone());
        self.models.entry(model.id().clone()).or_insert(model);
    }

    /// Returns the identity bound to the role `label`, when it was filled.
    #[must_use]
    pub fn role_id(&self, label: &str) -> Option<&ModelId> {
        self.roles.get(label)
    }

    /// Resolves a role label all the way to its descriptor:
    /// label -> id -> descriptor.
    #[must_use]
    pub fn resolve(&self, label: &str) -> Option<&ModelDescriptor> {
        self.roles.get(label).and_then(|id| self.models.get(id))
    }

    /// Returns the descriptor bound under `id`, when this run may use it.
    #[must_use]
    pub fn model(&self, id: &ModelId) -> Option<&ModelDescriptor> {
        self.models.get(id)
    }

    /// Returns the number of bound roles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.roles.len()
    }

    /// Returns whether no roles are bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }
}

/// The run's tool bindings: which concrete tool each declared alias is
/// bound to, and the tools this run may dispatch.
///
/// Written by [`prepare`](super::Environment::prepare)'s slot fill:
/// exact slots fill by identity against the assembled catalog and fuzzy
/// slots fill through the picker, and every fill is journaled here so
/// hosts and evals see what the fuzz resolved to. The model only ever
/// sees the prompt-local alias, never the global path. Handles resolve
/// alias -> id -> tool.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct ToolBindings {
    /// The decision, journaled: prompt-local alias to the bound tool's
    /// identity.
    aliases: BTreeMap<String, ToolId>,
    /// What this run may dispatch: identity to tool.
    tools: BTreeMap<ToolId, Arc<dyn Tool>>,
    /// Near-duplicate clashes per alias, recorded by the fill's conflict
    /// scan: the alias's bound tool is a near-verbatim copy of the named
    /// sibling alias's tool. Binding records, never fails: a clash errors
    /// only when both halves enter one model-visible scope.
    conflicts: BTreeMap<String, Vec<Conflict>>,
}

impl ToolBindings {
    /// Binds the prompt-local `alias` to `tool`, recording the tool under
    /// its identity. The slot fill's only writer.
    pub(crate) fn bind(&mut self, alias: &str, tool: Arc<dyn Tool>) {
        self.aliases.insert(alias.to_owned(), tool.id());
        self.tools.entry(tool.id()).or_insert(tool);
    }

    /// Returns the identity bound to `alias`, when the slot was filled.
    #[must_use]
    pub fn alias_id(&self, alias: &str) -> Option<&ToolId> {
        self.aliases.get(alias)
    }

    /// Resolves a prompt-local alias all the way to its tool:
    /// alias -> id -> tool.
    #[must_use]
    pub fn resolve(&self, alias: &str) -> Option<&Arc<dyn Tool>> {
        self.aliases.get(alias).and_then(|id| self.tools.get(id))
    }

    /// Returns the tool bound under `id`, when this run may dispatch it.
    #[must_use]
    pub fn tool(&self, id: &ToolId) -> Option<&Arc<dyn Tool>> {
        self.tools.get(id)
    }

    /// The distinct identities the fill bound, for the conflict scan.
    pub(crate) fn bound_ids(&self) -> Vec<ToolId> {
        self.tools.keys().cloned().collect()
    }

    /// Records one near-duplicate pair symmetrically: every alias bound
    /// to `first` clashes with every alias bound to `second`, and back.
    /// The score is the picker's cosine similarity, widened once at the
    /// scan.
    pub(crate) fn record_conflict(&mut self, first: &ToolId, second: &ToolId, similarity: f64) {
        let aliases_of = |id: &ToolId| -> Vec<String> {
            self.aliases
                .iter()
                .filter(|(_, bound)| *bound == id)
                .map(|(alias, _)| alias.clone())
                .collect()
        };
        let firsts = aliases_of(first);
        let seconds = aliases_of(second);
        for first_alias in &firsts {
            for second_alias in &seconds {
                self.conflicts
                    .entry(first_alias.clone())
                    .or_default()
                    .push(Conflict {
                        alias: second_alias.clone(),
                        similarity,
                    });
                self.conflicts
                    .entry(second_alias.clone())
                    .or_default()
                    .push(Conflict {
                        alias: first_alias.clone(),
                        similarity,
                    });
            }
        }
    }

    /// The near-duplicate clashes recorded against `alias` at the fill,
    /// empty when the conflict scan found none. Journaled with the
    /// bindings so hosts and evals see what the scan recorded.
    #[must_use]
    pub fn conflicts(&self, alias: &str) -> &[Conflict] {
        self.conflicts.get(alias).map_or(&[], Vec::as_slice)
    }

    /// Returns the number of bound aliases.
    #[must_use]
    pub fn len(&self) -> usize {
        self.aliases.len()
    }

    /// Returns whether no aliases are bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty()
    }
}

impl fmt::Debug for ToolBindings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The tools are trait objects; their identities stand in, and
        // the journaled decision (alias to identity, recorded clashes)
        // is the content.
        f.debug_struct("ToolBindings")
            .field("aliases", &self.aliases)
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .field("conflicts", &self.conflicts)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::sync::Arc;

    use promptforge_api_types::tools::{ToolError, ToolOutput};

    use super::*;
    use crate::model::ThinkingMode;

    fn descriptor(name: &str) -> ModelDescriptor {
        ModelDescriptor::new(
            ModelId::gateway(name).expect("the test id is valid"),
            "A test model",
            NonZeroU32::new(32_000).expect("the context window is non-zero"),
            ThinkingMode::Switchable,
        )
    }

    #[test]
    fn an_empty_binding_set_resolves_nothing() {
        let bindings = ModelBindings::default();
        assert!(bindings.is_empty());
        assert_eq!(bindings.len(), 0);
        assert!(bindings.role_id("analyst").is_none());
        assert!(bindings.resolve("analyst").is_none());
    }

    #[test]
    fn two_roles_bound_to_one_model_share_one_descriptor_entry() {
        // v1's trivial fill: every role binds the same model, and the
        // descriptor table holds it once - the seam a smarter fill grows
        // into is visible in the shape, not the content.
        let model = descriptor("current");
        let mut bindings = ModelBindings::default();
        bindings.bind("analyst", model.clone());
        bindings.bind("triage", model.clone());
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings.role_id("analyst"), Some(model.id()));
        assert_eq!(bindings.role_id("triage"), Some(model.id()));
        assert_eq!(bindings.resolve("analyst"), Some(&model));
        assert_eq!(bindings.resolve("triage"), Some(&model));
        assert_eq!(bindings.model(model.id()), Some(&model));
        assert!(
            bindings
                .model(&ModelId::gateway("other").expect("valid"))
                .is_none()
        );
    }

    /// A fixture tool: a static id and an empty trusted output.
    struct FixtureTool {
        id: ToolId,
    }

    #[async_trait::async_trait]
    impl Tool for FixtureTool {
        fn id(&self) -> ToolId {
            self.id.clone()
        }

        fn wire_name(&self) -> &'static str {
            "fixture"
        }

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn description(&self) -> &str {
            "A fixture tool."
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }

        async fn call(
            &self,
            _arguments: serde_json::Value,
        ) -> std::result::Result<ToolOutput, ToolError> {
            Ok(ToolOutput::trusted(String::new()))
        }
    }

    #[test]
    fn an_empty_tool_binding_set_resolves_nothing() {
        let bindings = ToolBindings::default();
        assert!(bindings.is_empty());
        assert_eq!(bindings.len(), 0);
        assert!(bindings.alias_id("fetch").is_none());
        assert!(bindings.resolve("fetch").is_none());
    }

    #[test]
    fn two_aliases_bound_to_one_tool_share_one_tool_entry() {
        // Two slots may fill to the same tool; the tool table holds it
        // once and both aliases resolve alias -> id -> tool.
        let id = ToolId::parse("promptforge/web/fetch").expect("the test id is valid");
        let tool: Arc<dyn Tool> = Arc::new(FixtureTool { id: id.clone() });
        let mut bindings = ToolBindings::default();
        bindings.bind("fetch", Arc::clone(&tool));
        bindings.bind("getter", Arc::clone(&tool));
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings.alias_id("fetch"), Some(&id));
        assert_eq!(bindings.alias_id("getter"), Some(&id));
        assert_eq!(
            bindings.resolve("fetch").map(|tool| tool.id()),
            Some(id.clone())
        );
        assert_eq!(
            bindings.resolve("getter").map(|tool| tool.id()),
            Some(id.clone())
        );
        assert!(bindings.tool(&id).is_some());
        assert!(
            bindings
                .tool(&ToolId::parse("promptforge/web/search").expect("valid"))
                .is_none()
        );
    }

    #[test]
    fn a_recorded_conflict_lands_on_both_aliases_symmetrically() {
        // The fill's conflict scan records a near-duplicate pair on every
        // alias bound to each half, so the scope check fires whichever
        // alias pair enters one model-visible scope.
        let first = ToolId::parse("promptforge/web/fetch").expect("the test id is valid");
        let second = ToolId::parse("promptforge/web/getter").expect("the test id is valid");
        let mut bindings = ToolBindings::default();
        bindings.bind("fetch", Arc::new(FixtureTool { id: first.clone() }));
        bindings.bind("getter", Arc::new(FixtureTool { id: second.clone() }));
        assert!(bindings.conflicts("fetch").is_empty());
        bindings.record_conflict(&first, &second, 0.97);
        let fetch_conflicts = bindings.conflicts("fetch");
        assert_eq!(fetch_conflicts.len(), 1);
        assert_eq!(fetch_conflicts[0].alias, "getter");
        assert!((fetch_conflicts[0].similarity - 0.97).abs() < f64::EPSILON);
        let getter_conflicts = bindings.conflicts("getter");
        assert_eq!(getter_conflicts.len(), 1);
        assert_eq!(getter_conflicts[0].alias, "fetch");
        assert!(bindings.conflicts("unbound").is_empty());
    }
}
