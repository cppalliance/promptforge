//! Run-scoped live capability resolution for H1 execution.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use mlua::{Lua, Scope};
#[cfg(test)]
use promptforge_tool_picker::ToolId as PickerToolId;
use promptforge_tool_picker::{Outcome, ToolDescriptor, ToolPicker};

use crate::error::SharedSource;
use crate::lua::{LiveBindingProducer, ToolBindings, ToolResolver};
use crate::model::{
    ModelBindOpts, ModelBindings, ModelCatalog, ModelResolver, PickerModelResolver, ResolvedModel,
};
use crate::tools::{ToolCatalog, ToolId};
use crate::{Error, Result};

/// Run-scoped capability resolver and live H1 binding producer.
pub(crate) struct RuntimeResolution<'a> {
    tool_resolver: PickerResolver<'a, ToolPicker>,
    tools: &'a ToolCatalog,
    models: &'a ModelCatalog,
    base_picker: &'a ToolPicker,
    producer: LiveBindingProducer,
}

impl<'a> RuntimeResolution<'a> {
    /// Creates one run-scoped resolver over live tool and model catalogs.
    ///
    /// The tool catalog already guarantees unique tool identities
    /// (duplicates are rejected at construction), so no identity scan is
    /// needed here.
    ///
    /// Construction retains only the base picker/embedder (F7): it does NOT
    /// pre-build a full model index that model resolution would immediately
    /// discard and rebuild from the constraint-filtered subset. The filtered
    /// model index is built on demand, when a `models.bind`'s constraints are
    /// known, so the redundant full-catalog index is never materialized.
    pub(crate) fn new(
        picker: &'a ToolPicker,
        tools: &'a ToolCatalog,
        models: &'a ModelCatalog,
    ) -> Self {
        Self {
            tool_resolver: PickerResolver::new(picker),
            tools,
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
            .install(lua, scope, &self.tool_resolver, self.tools, self)
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

impl ModelResolver for RuntimeResolution<'_> {
    fn resolve(&self, description: &str, opts: &ModelBindOpts) -> Result<ResolvedModel> {
        // An empty catalog resolves every bind as absent without touching the
        // picker at all.
        if self.models.is_empty() {
            return Err(Error::ModelAbsent {
                capability: description.to_owned(),
            });
        }
        // The filtered model index is built here, from the base embedder, over
        // just the descriptors that satisfy the bind's constraints (F7).
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
    /// The picker's query failed. The typed
    /// [`promptforge_tool_picker::QueryError`] is retained as a shareable source
    /// (F4) so the failure chain survives the cache; it is wrapped once here and
    /// cloned (an `Arc` bump) into a fresh `Error` on every cache hit.
    QueryFailed(SharedSource),
    /// The picker returned an outcome this resolver does not model (a defensive
    /// catch-all; no dependency error to preserve).
    Unrecognized,
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
            Ok(_) => Self::Unrecognized,
            Err(error) => Self::QueryFailed(SharedSource::new(error)),
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
            Self::QueryFailed(source) => Err(Error::BindQuery {
                capability: capability.to_owned(),
                source: source.clone(),
            }),
            Self::Unrecognized => Err(Error::Bind {
                capability: capability.to_owned(),
                detail: "the picker reported an unrecognized outcome".to_owned(),
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

/// One cached, single-flight decision cell for a capability (F1).
type DecisionCell = Arc<OnceLock<CachedDecision>>;

#[derive(Debug)]
struct PickerResolver<'a, S: ?Sized> {
    source: &'a S,
    /// Per-capability decision cache. Each entry is a per-key
    /// [`OnceLock`] cell so a concurrent miss for one capability runs
    /// [`DecisionSource::decide`] exactly once (single-flight, F1); the global
    /// map lock is only held to fetch or insert the cell, never across the
    /// expensive query. Holds only normalized outcomes (F2: the former
    /// write-only diagnostics map, whose sole reader was a test, is gone).
    decisions: Mutex<BTreeMap<String, DecisionCell>>,
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
    fn lock_decisions(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, DecisionCell>>> {
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
        // Fetch or create this capability's single-flight cell under a short
        // lock that touches only the map, never the picker query.
        let cell = {
            let mut decisions = self.lock_decisions()?;
            Arc::clone(
                decisions
                    .entry(capability.to_owned())
                    .or_insert_with(|| Arc::new(OnceLock::new())),
            )
        };
        // Single-flight (F1): the first caller to reach an uninitialized cell
        // runs the (potentially expensive, re-entrant) picker query exactly
        // once; concurrent callers for the SAME capability block on this cell
        // until that result is published, then all observe the identical
        // decision. Different capabilities hold different cells, so unrelated
        // misses never serialize, and the global map lock is not held across
        // the query.
        let decision = cell.get_or_init(|| self.source.decide(capability));
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
    use crate::model::ModelBindOpts;
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
                other => CachedDecision::QueryFailed(SharedSource::new(std::io::Error::other(
                    format!("picker failed for {other}"),
                ))),
            }
        }

        fn near_duplicates(
            &self,
            ids: &[PickerToolId],
        ) -> std::result::Result<Vec<(PickerToolId, PickerToolId, f32)>, String> {
            Ok(vec![(ids[0].clone(), ids[1].clone(), 0.97)])
        }
    }

    #[test]
    fn concurrent_misses_run_decide_once_per_capability() {
        // F1: many threads racing on the SAME capability must run the expensive
        // `decide` exactly once (single-flight), and every racer must observe
        // the identical published decision.
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingSource {
            calls: AtomicUsize,
        }
        impl DecisionSource for CountingSource {
            fn decide(&self, capability: &str) -> CachedDecision {
                self.calls.fetch_add(1, Ordering::SeqCst);
                // Simulate an expensive re-entrant query so racers overlap on
                // the uninitialized cell.
                std::thread::sleep(std::time::Duration::from_millis(25));
                CachedDecision::Bind(tid(capability))
            }

            fn near_duplicates(
                &self,
                ids: &[PickerToolId],
            ) -> std::result::Result<Vec<(PickerToolId, PickerToolId, f32)>, String> {
                Ok(vec![(ids[0].clone(), ids[1].clone(), 0.0)])
            }
        }

        let source = CountingSource {
            calls: AtomicUsize::new(0),
        };
        let resolver = PickerResolver::new(&source);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| resolver.resolve("same").map(|id| id.name().to_owned())))
                .collect();
            for handle in handles {
                assert_eq!(handle.join().expect("thread joins").expect("bound"), "same");
            }
        });
        assert_eq!(
            source.calls.load(Ordering::SeqCst),
            1,
            "decide must run exactly once per capability under concurrent misses"
        );
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
        let catalog = ToolCatalog::new(tools).expect("fixture tools are unique");
        let producer = LiveBindingProducer::default();
        let model_resolver = |description: &str, _: &ModelBindOpts| {
            Err(Error::ModelAbsent {
                capability: description.to_owned(),
            })
        };
        let lua = Lua::new();
        let result = lua.scope(|scope| {
            producer
                .install(&lua, scope, &resolver, &catalog, &model_resolver)
                .map_err(mlua::Error::external)?;
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
        // F4: a picker query failure keeps the typed cause as a private
        // `#[source]` rather than flattening it into a string.
        let query_failed = CachedDecision::QueryFailed(SharedSource::new(std::io::Error::other(
            "embedding backend down",
        )))
        .result("failed")
        .expect_err("a query failure must be an error");
        assert!(matches!(
            &query_failed,
            Error::BindQuery { capability, .. } if capability == "failed"
        ));
        let source = std::error::Error::source(&query_failed).expect("cause preserved");
        assert!(
            source.to_string().contains("embedding backend down"),
            "the picker cause must survive as a source, got {source}"
        );

        // The defensive unrecognized-outcome decision maps to a sourceless bind.
        assert!(matches!(
            CachedDecision::Unrecognized.result("weird"),
            Err(Error::Bind { capability, detail })
                if capability == "weird" && detail.contains("unrecognized")
        ));
    }

    #[test]
    fn callback_boundary_retains_absent_and_missing_catalog_errors() {
        assert!(matches!(
            callback_error(
                &FixtureSource,
                &[],
                "tools.bind('missing', 'absent')"
            ),
            Error::Absent { capability } if capability == "absent"
        ));
        assert!(matches!(
            callback_error(
                &FixtureSource,
                &[],
                "tools.bind('missing', 'first')"
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
                "tools.bind('same', 'first'); tools.bind('same', 'first')"
            ),
            Error::DuplicateAlias { alias } if alias == "same"
        ));
        assert!(matches!(
            callback_error(
                &FixtureSource,
                &tools,
                "tools.bind('one', 'same-one'); tools.bind('two', 'same-two')"
            ),
            Error::ToolIdSelectedTwice { id, first_alias, second_alias }
                if id == ToolId::new("tests", "first").expect("valid id")
                    && first_alias == "one"
                    && second_alias == "two"
        ));
    }

    #[test]
    fn catalog_rejects_duplicate_live_ids() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(FixtureTool {
                id: ToolId::new("tests", "same").expect("valid id"),
            }),
            Arc::new(FixtureTool {
                id: ToolId::new("tests", "same").expect("valid id"),
            }),
        ];
        let error = ToolCatalog::new(&tools)
            .expect_err("a repeated live identity must be rejected at catalog construction");
        assert_eq!(
            error.duplicate_id(),
            Some(&ToolId::new("tests", "same").expect("valid id"))
        );
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
        assert_eq!(pairs[0].0, ids[0]);
        assert_eq!(pairs[0].1, ids[1]);
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
