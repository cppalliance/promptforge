//! Run-scoped live capability resolution for H1 execution.

use std::collections::BTreeMap;
use std::sync::Mutex;

use mlua::{Lua, Scope};
#[cfg(test)]
use promptforge_tool_picker::ToolId as PickerToolId;
use promptforge_tool_picker::{Outcome, ToolDescriptor, ToolPicker};

use crate::lua::{LiveBindingProducer, ToolBindings, ToolResolver};
use crate::model::{
    ModelBindings, ModelCatalog, ModelNeedOpts, ModelResolver, PickerModelResolver, ResolvedModel,
};
use crate::tools::{ToolId, ToolRegistry};
use crate::{Error, Result};

/// Run-scoped capability resolver and live H1 binding producer.
pub(crate) struct RuntimeResolution<'a, 'tools: 'a> {
    tool_resolver: PickerResolver<'a, ToolPicker>,
    registry: &'a ToolRegistry<'tools>,
    models: &'a ModelCatalog,
    base_picker: &'a ToolPicker,
    producer: LiveBindingProducer,
}

impl<'a, 'tools: 'a> RuntimeResolution<'a, 'tools> {
    /// Creates one run-scoped resolver over live tool and model catalogs.
    ///
    /// The `registry` already guarantees unique tool identities (duplicates are
    /// rejected at registration), so no identity scan is needed here.
    ///
    /// Construction retains only the base picker/embedder (F7): it does NOT
    /// pre-build a full model index that model resolution would immediately
    /// discard and rebuild from the constraint-filtered subset. The filtered
    /// model index is built on demand, when a `models.need`'s constraints are
    /// known, so the redundant full-catalog index is never materialized.
    pub(crate) fn new(
        picker: &'a ToolPicker,
        registry: &'a ToolRegistry<'tools>,
        models: &'a ModelCatalog,
    ) -> Self {
        Self {
            tool_resolver: PickerResolver::new(picker),
            registry,
            models,
            base_picker: picker,
            producer: LiveBindingProducer::default(),
        }
    }

    /// Installs call-time tool and model resolution into an H1 Lua scope.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the resolver tables cannot be installed.
    pub(crate) fn install<'scope, 'env: 'scope>(
        &'env self,
        lua: &'env Lua,
        scope: &'scope Scope<'scope, 'env>,
    ) -> Result<()> {
        self.producer
            .install(lua, scope, &self.tool_resolver, self.registry, self)
    }

    /// Returns the first typed error captured by a resolver callback.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if a binding recorder mutex is poisoned.
    pub(crate) fn take_callback_error(&self) -> Result<Option<Error>> {
        self.producer.take_callback_error()
    }

    /// Snapshots the tool and model bindings resolved by executed H1 code.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if a binding recorder mutex is poisoned.
    pub(crate) fn bindings(&self) -> Result<(ToolBindings, ModelBindings)> {
        self.producer.bindings()
    }

    /// Returns a shared handle to bindings resolved by live H1 so far.
    ///
    /// `#[must_use]`: the returned clone is this call's sole effect (F6), so
    /// discarding it silently drops the snapshot handle the caller asked for.
    #[must_use]
    pub(crate) fn producer(&self) -> LiveBindingProducer {
        self.producer.clone()
    }
}

impl ModelResolver for RuntimeResolution<'_, '_> {
    fn resolve(&self, description: &str, opts: &ModelNeedOpts) -> Result<ResolvedModel> {
        // An empty catalog resolves every need as absent without touching the
        // picker at all.
        if self.models.is_empty() {
            return Err(Error::ModelAbsent {
                capability: description.to_owned(),
            });
        }
        // The filtered model index is built here, from the base embedder, over
        // just the descriptors that satisfy the need's constraints (F7).
        PickerModelResolver::new(self.models, self.base_picker).resolve(description, opts)
    }
}

/// A resolved capability outcome, normalized once into core-owned identities.
///
/// Picker [`ToolDescriptor`]s are converted to core [`ToolId`]s at decision
/// time (F4), so a cached decision holds only the stable identities the caller
/// needs; a cache hit produces its typed result from these borrowed ids without
/// re-cloning full descriptors on every resolve.
#[derive(Debug)]
enum CachedDecision {
    Bind(ToolId),
    Absent,
    Duplicate(Vec<ToolId>),
    Ambiguous(Vec<ToolId>),
    Failed(String),
}

/// Converts a borrowed picker descriptor to a core-owned [`ToolId`].
fn tool_id_of(tool: &ToolDescriptor) -> ToolId {
    ToolId::from_validated(tool.id().server(), tool.id().name())
}

impl CachedDecision {
    fn from_picker(
        outcome: std::result::Result<Outcome<'_>, promptforge_tool_picker::QueryError>,
    ) -> Self {
        match outcome {
            Ok(Outcome::Bind(tool)) => Self::Bind(tool_id_of(tool)),
            Ok(Outcome::Absent) => Self::Absent,
            Ok(Outcome::Duplicate(group)) => {
                Self::Duplicate(group.iter().map(tool_id_of).collect())
            }
            Ok(Outcome::Ambiguous(group)) => {
                Self::Ambiguous(group.iter().map(tool_id_of).collect())
            }
            Ok(_) => Self::Failed("the picker reported an unrecognized outcome".to_owned()),
            Err(error) => Self::Failed(error.to_string()),
        }
    }

    fn result(&self, capability: &str) -> Result<ToolId> {
        match self {
            Self::Bind(id) => Ok(id.clone()),
            Self::Absent => Err(Error::Absent {
                capability: capability.to_owned(),
            }),
            Self::Duplicate(ids) => Err(Error::Duplicate {
                capability: capability.to_owned(),
                candidates: ids.clone(),
            }),
            Self::Ambiguous(ids) => Err(Error::Ambiguous {
                capability: capability.to_owned(),
                candidates: ids.clone(),
            }),
            Self::Failed(detail) => Err(Error::Bind {
                capability: capability.to_owned(),
                detail: detail.clone(),
            }),
        }
    }
}

trait DecisionSource: Send + Sync {
    fn decide(&self, capability: &str) -> CachedDecision;

    #[cfg(test)]
    fn near_duplicates(
        &self,
        ids: &[PickerToolId],
    ) -> std::result::Result<Vec<(PickerToolId, PickerToolId, f32)>, String>;
}

impl DecisionSource for ToolPicker {
    fn decide(&self, capability: &str) -> CachedDecision {
        CachedDecision::from_picker(self.resolve(capability))
    }

    #[cfg(test)]
    fn near_duplicates(
        &self,
        ids: &[PickerToolId],
    ) -> std::result::Result<Vec<(PickerToolId, PickerToolId, f32)>, String> {
        ToolPicker::near_duplicates(self, ids)
            .map(|pairs| {
                pairs
                    .iter()
                    .map(|pair| {
                        (
                            pair.first().id().clone(),
                            pair.second().id().clone(),
                            pair.similarity(),
                        )
                    })
                    .collect()
            })
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug)]
struct PickerResolver<'a, S: ?Sized> {
    source: &'a S,
    /// Per-capability decision cache. Holds only normalized outcomes (F2: the
    /// former write-only diagnostics map, whose sole reader was a test, is gone).
    decisions: Mutex<BTreeMap<String, CachedDecision>>,
}

impl<'a, S: ?Sized> PickerResolver<'a, S> {
    fn new(source: &'a S) -> Self {
        Self {
            source,
            decisions: Mutex::new(BTreeMap::new()),
        }
    }

    /// Locks the decision cache, mapping a poisoned lock to a resolver-state
    /// error (F3) rather than mislabeling it as a Lua authoring failure.
    fn lock_decisions(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, CachedDecision>>> {
        self.decisions
            .lock()
            .map_err(|_| Error::Internal("tool picker resolver cache was poisoned"))
    }
}

impl<S> ToolResolver for PickerResolver<'_, S>
where
    S: DecisionSource + ?Sized,
{
    fn resolve(&self, capability: &str) -> Result<ToolId> {
        // Fast path: a short lock that only reads an already-computed decision.
        {
            let decisions = self.lock_decisions()?;
            if let Some(decision) = decisions.get(capability) {
                return decision.result(capability);
            }
        }
        // Compute the (potentially expensive, re-entrant) decision OUTSIDE the
        // lock so unrelated capability misses do not serialize behind one
        // capability's picker query (F1).
        let computed = self.source.decide(capability);
        // Publish under a short lock. If another thread raced us to the same
        // key, keep the already-published decision so repeated lookups return a
        // stable cached outcome.
        let mut decisions = self.lock_decisions()?;
        let decision = decisions.entry(capability.to_owned()).or_insert(computed);
        decision.result(capability)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mlua::Lua;
    use serde_json::{Value, json};

    use super::*;
    use crate::lua::LiveBindingProducer;
    use crate::model::ModelNeedOpts;
    use crate::tools::{Tool, ToolError, ToolOutput};

    fn tid(name: &str) -> ToolId {
        ToolId::from_validated("tests", name)
    }

    struct FixtureSource;

    impl DecisionSource for FixtureSource {
        fn decide(&self, capability: &str) -> CachedDecision {
            match capability {
                "first" | "same-one" | "same-two" => CachedDecision::Bind(tid("first")),
                "second" => CachedDecision::Bind(tid("second")),
                "absent" => CachedDecision::Absent,
                "duplicate" => CachedDecision::Duplicate(vec![tid("first"), tid("second")]),
                "ambiguous" => CachedDecision::Ambiguous(vec![tid("first"), tid("second")]),
                other => CachedDecision::Failed(format!("picker failed for {other}")),
            }
        }

        fn near_duplicates(
            &self,
            ids: &[PickerToolId],
        ) -> std::result::Result<Vec<(PickerToolId, PickerToolId, f32)>, String> {
            Ok(vec![(ids[0].clone(), ids[1].clone(), 0.97)])
        }
    }

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

        fn description(&self) -> &'static str {
            "fixture"
        }

        fn parameters_schema(&self) -> Value {
            json!({})
        }

        async fn call(&self, _arguments: Value) -> std::result::Result<ToolOutput, ToolError> {
            Ok(ToolOutput::trusted(String::new()))
        }
    }

    fn callback_error(source: &FixtureSource, tools: &[Arc<dyn Tool>], code: &str) -> Error {
        let resolver = PickerResolver::new(source);
        let registry =
            ToolRegistry::new(tools.iter().map(AsRef::as_ref)).expect("fixture tools are unique");
        let producer = LiveBindingProducer::default();
        let model_resolver = |description: &str, _: &ModelNeedOpts| {
            Err(Error::ModelAbsent {
                capability: description.to_owned(),
            })
        };
        let lua = Lua::new();
        let result = lua.scope(|scope| {
            producer
                .install(&lua, scope, &resolver, &registry, &model_resolver)
                .map_err(|error| mlua::Error::external(error.to_string()))?;
            lua.load(code).exec()
        });
        assert!(result.is_err(), "fixture must fail at the Lua callback");
        producer
            .take_callback_error()
            .expect("callback recorder must remain usable")
            .expect("typed callback error must be retained")
    }

    #[test]
    fn picker_outcomes_preserve_typed_errors_and_candidate_order() {
        let duplicate = CachedDecision::Duplicate(vec![tid("first"), tid("second")])
            .result("duplicate")
            .expect_err("duplicate must fail");
        assert!(matches!(
            duplicate,
            Error::Duplicate { capability, candidates }
                if capability == "duplicate"
                    && candidates == [
                        ToolId::new("tests", "first").expect("valid id"),
                        ToolId::new("tests", "second").expect("valid id")
                    ]
        ));
        assert!(matches!(
            CachedDecision::Absent.result("absent"),
            Err(Error::Absent { capability }) if capability == "absent"
        ));
        assert!(matches!(
            CachedDecision::Ambiguous(vec![tid("first"), tid("second")]).result("ambiguous"),
            Err(Error::Ambiguous { capability, candidates })
                if capability == "ambiguous" && candidates.len() == 2
        ));
        assert!(matches!(
            CachedDecision::Failed("private failure".to_owned()).result("failed"),
            Err(Error::Bind { capability, detail })
                if capability == "failed" && detail == "private failure"
        ));
    }

    #[test]
    fn callback_boundary_retains_absent_and_missing_registry_errors() {
        assert!(matches!(
            callback_error(
                &FixtureSource,
                &[],
                "tools.need('missing', 'absent')"
            ),
            Error::Absent { capability } if capability == "absent"
        ));
        assert!(matches!(
            callback_error(
                &FixtureSource,
                &[],
                "tools.need('missing', 'first')"
            ),
            Error::PickedToolNotLive { alias, id }
                if alias == "missing" && id == ToolId::new("tests", "first").expect("valid id")
        ));
    }

    #[test]
    fn live_callbacks_reject_duplicate_aliases_and_identities() {
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FixtureTool {
            id: ToolId::new("tests", "first").expect("valid id"),
        })];
        assert!(matches!(
            callback_error(
                &FixtureSource,
                &tools,
                "tools.need('same', 'first'); tools.need('same', 'first')"
            ),
            Error::DuplicateAlias { alias } if alias == "same"
        ));
        assert!(matches!(
            callback_error(
                &FixtureSource,
                &tools,
                "tools.need('one', 'same-one'); tools.need('two', 'same-two')"
            ),
            Error::ToolIdSelectedTwice { id, first_alias, second_alias }
                if id == ToolId::new("tests", "first").expect("valid id")
                    && first_alias == "one"
                    && second_alias == "two"
        ));
    }

    #[test]
    fn registration_rejects_duplicate_live_registry_ids() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(FixtureTool {
                id: ToolId::new("tests", "same").expect("valid id"),
            }),
            Arc::new(FixtureTool {
                id: ToolId::new("tests", "same").expect("valid id"),
            }),
        ];
        let error = ToolRegistry::new(tools.iter().map(AsRef::as_ref))
            .expect_err("a repeated live identity must be rejected at registration");
        assert_eq!(error.id(), &ToolId::new("tests", "same").expect("valid id"));
    }

    #[test]
    fn near_duplicates_are_forwarded_from_the_source() {
        let ids = [
            PickerToolId::new("tests", "first"),
            PickerToolId::new("tests", "second"),
        ];
        let pairs = FixtureSource
            .near_duplicates(&ids)
            .expect("analysis succeeds");
        assert_eq!(pairs.len(), 1);
        assert!((pairs[0].2 - 0.97).abs() < f32::EPSILON);
    }

    /// A decision source that counts how many times each capability is decided,
    /// so a test can prove the resolver caches (decides at most once) and does
    /// not re-query the picker on repeated hits (F5).
    struct CountingSource {
        counts: Mutex<BTreeMap<String, usize>>,
    }

    impl CountingSource {
        fn new() -> Self {
            Self {
                counts: Mutex::new(BTreeMap::new()),
            }
        }

        fn count(&self, capability: &str) -> usize {
            self.counts
                .lock()
                .expect("counts lock")
                .get(capability)
                .copied()
                .unwrap_or(0)
        }
    }

    impl DecisionSource for CountingSource {
        fn decide(&self, capability: &str) -> CachedDecision {
            *self
                .counts
                .lock()
                .expect("counts lock")
                .entry(capability.to_owned())
                .or_insert(0) += 1;
            FixtureSource.decide(capability)
        }

        fn near_duplicates(
            &self,
            _ids: &[PickerToolId],
        ) -> std::result::Result<Vec<(PickerToolId, PickerToolId, f32)>, String> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn each_capability_is_decided_once_and_returns_a_stable_cached_outcome() {
        let source = CountingSource::new();
        let resolver = PickerResolver::new(&source);

        // A successful capability, resolved repeatedly, is decided exactly once
        // and returns the same identity every time.
        let first_a = resolver.resolve("first").expect("first resolves");
        let first_b = resolver.resolve("first").expect("first resolves again");
        assert_eq!(first_a, first_b);
        assert_eq!(first_a, ToolId::new("tests", "first").expect("valid id"));
        assert_eq!(source.count("first"), 1, "a hit must not re-decide");

        // A failing capability is likewise cached: decided once, stable error.
        let miss_a = resolver.resolve("absent").expect_err("absent fails");
        let miss_b = resolver.resolve("absent").expect_err("absent fails again");
        assert!(matches!(miss_a, Error::Absent { .. }));
        assert!(matches!(miss_b, Error::Absent { .. }));
        assert_eq!(
            source.count("absent"),
            1,
            "a cached miss must not re-decide"
        );

        // A distinct capability is decided on its own miss.
        resolver.resolve("second").expect("second resolves");
        assert_eq!(source.count("second"), 1);
        assert_eq!(source.count("first"), 1);
    }
}
