//! Run-scoped live capability resolution for H1 execution.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use mlua::{Lua, Scope};
#[cfg(test)]
use promptforge_tool_picker::ToolId as PickerToolId;
use promptforge_tool_picker::{Outcome, ToolDescriptor, ToolPicker};

use crate::lua::{LiveBindingProducer, ToolBindings, ToolResolver};
use crate::model::{
    ModelBindings, ModelCatalog, ModelNeedOpts, ModelResolver, PickerModelResolver, ResolvedModel,
    model_picker_from,
};
use crate::tools::{ToolId, ToolRegistry};
use crate::{Error, Result};

/// Run-scoped capability resolver and live H1 binding producer.
pub(crate) struct RuntimeResolution<'a, 'tools: 'a> {
    tool_resolver: PickerResolver<'a, ToolPicker>,
    registry: &'a ToolRegistry<'tools>,
    models: &'a ModelCatalog,
    model_picker: Option<ToolPicker>,
    producer: LiveBindingProducer,
}

impl<'a, 'tools: 'a> RuntimeResolution<'a, 'tools> {
    /// Creates one run-scoped resolver over live tool and model catalogs.
    ///
    /// # Errors
    /// Returns [`Error::DuplicateLiveToolId`] when the registry repeats an
    /// identity, or [`Error::ModelBind`] when the model picker cannot be built.
    pub(crate) fn new(
        picker: &'a ToolPicker,
        registry: &'a ToolRegistry<'tools>,
        models: &'a ModelCatalog,
    ) -> Result<Self> {
        let mut live_ids = BTreeSet::new();
        for tool in registry.tools() {
            let id = tool.id();
            if !live_ids.insert(id.clone()) {
                return Err(Error::DuplicateLiveToolId { id });
            }
        }
        let model_picker = if models.is_empty() {
            None
        } else {
            Some(model_picker_from(picker, models)?)
        };
        Ok(Self {
            tool_resolver: PickerResolver::new(picker),
            registry,
            models,
            model_picker,
            producer: LiveBindingProducer::default(),
        })
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
    #[must_use]
    pub(crate) fn producer(&self) -> LiveBindingProducer {
        self.producer.clone()
    }
}

impl ModelResolver for RuntimeResolution<'_, '_> {
    fn resolve(&self, description: &str, opts: &ModelNeedOpts) -> Result<ResolvedModel> {
        let Some(picker) = self.model_picker.as_ref() else {
            return Err(Error::ModelAbsent {
                capability: description.to_owned(),
            });
        };
        PickerModelResolver::new(self.models, picker).resolve(description, opts)
    }
}

#[derive(Debug, Clone)]
enum CachedDecision {
    Bind(ToolDescriptor),
    Absent,
    Duplicate(Vec<ToolDescriptor>),
    Ambiguous(Vec<ToolDescriptor>),
    Failed(String),
}

impl CachedDecision {
    fn from_picker(
        outcome: std::result::Result<Outcome<'_>, promptforge_tool_picker::QueryError>,
    ) -> Self {
        match outcome {
            Ok(Outcome::Bind(tool)) => Self::Bind(tool.clone()),
            Ok(Outcome::Absent) => Self::Absent,
            Ok(Outcome::Duplicate(tools)) => Self::Duplicate(tools.iter().cloned().collect()),
            Ok(Outcome::Ambiguous(tools)) => Self::Ambiguous(tools.iter().cloned().collect()),
            Err(error) => Self::Failed(error.to_string()),
            Ok(_) => Self::Failed("unsupported tool-picker outcome".to_owned()),
        }
    }

    fn result(&self, capability: &str) -> Result<ToolId> {
        match self {
            Self::Bind(tool) => Ok(ToolId::new(tool.id().server(), tool.id().name())),
            Self::Absent => Err(Error::Absent {
                capability: capability.to_owned(),
            }),
            Self::Duplicate(tools) => Err(Error::Duplicate {
                capability: capability.to_owned(),
                candidates: tools
                    .iter()
                    .map(|tool| ToolId::new(tool.id().server(), tool.id().name()))
                    .collect(),
            }),
            Self::Ambiguous(tools) => Err(Error::Ambiguous {
                capability: capability.to_owned(),
                candidates: tools
                    .iter()
                    .map(|tool| ToolId::new(tool.id().server(), tool.id().name()))
                    .collect(),
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
    ) -> std::result::Result<Vec<(ToolDescriptor, ToolDescriptor, f32)>, String>;
}

impl DecisionSource for ToolPicker {
    fn decide(&self, capability: &str) -> CachedDecision {
        CachedDecision::from_picker(self.resolve(capability))
    }

    #[cfg(test)]
    fn near_duplicates(
        &self,
        ids: &[PickerToolId],
    ) -> std::result::Result<Vec<(ToolDescriptor, ToolDescriptor, f32)>, String> {
        ToolPicker::near_duplicates(self, ids)
            .map(|pairs| {
                pairs
                    .iter()
                    .map(|pair| {
                        (
                            pair.first().clone(),
                            pair.second().clone(),
                            pair.similarity(),
                        )
                    })
                    .collect()
            })
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug)]
struct ResolverState {
    decisions: BTreeMap<String, CachedDecision>,
    diagnostics: BTreeMap<ToolId, ToolDescriptor>,
}

#[derive(Debug)]
struct PickerResolver<'a, S: ?Sized> {
    source: &'a S,
    state: Mutex<ResolverState>,
}

impl<'a, S: ?Sized> PickerResolver<'a, S> {
    fn new(source: &'a S) -> Self {
        Self {
            source,
            state: Mutex::new(ResolverState {
                decisions: BTreeMap::new(),
                diagnostics: BTreeMap::new(),
            }),
        }
    }

    #[cfg(test)]
    fn diagnostics(self) -> Result<BTreeMap<ToolId, ToolDescriptor>> {
        self.state
            .into_inner()
            .map(|state| state.diagnostics)
            .map_err(|_| Error::Lua("tool picker cache was poisoned".to_owned()))
    }
}

impl<S> ToolResolver for PickerResolver<'_, S>
where
    S: DecisionSource + ?Sized,
{
    fn resolve(&self, capability: &str) -> Result<ToolId> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::Lua("tool picker cache was poisoned".to_owned()))?;
        let decision = state
            .decisions
            .entry(capability.to_owned())
            .or_insert_with(|| self.source.decide(capability))
            .clone();
        if let CachedDecision::Bind(tool) = &decision {
            state.diagnostics.insert(
                ToolId::new(tool.id().server(), tool.id().name()),
                tool.clone(),
            );
        }
        decision.result(capability)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mlua::Lua;
    use promptforge_tool_picker::{Catalog, Config};
    use serde_json::{Value, json};

    use super::*;
    use crate::lua::LiveBindingProducer;
    use crate::model::ModelNeedOpts;
    use crate::tools::Tool;

    fn descriptor(name: &str) -> ToolDescriptor {
        ToolDescriptor::new(
            PickerToolId::new("tests", name),
            format!("{name} capability"),
            json!({}),
        )
    }

    struct FixtureSource;

    impl DecisionSource for FixtureSource {
        fn decide(&self, capability: &str) -> CachedDecision {
            match capability {
                "first" | "same-one" | "same-two" => CachedDecision::Bind(descriptor("first")),
                "second" => CachedDecision::Bind(descriptor("second")),
                "absent" => CachedDecision::Absent,
                "duplicate" => {
                    CachedDecision::Duplicate(vec![descriptor("first"), descriptor("second")])
                }
                "ambiguous" => {
                    CachedDecision::Ambiguous(vec![descriptor("first"), descriptor("second")])
                }
                other => CachedDecision::Failed(format!("picker failed for {other}")),
            }
        }

        fn near_duplicates(
            &self,
            ids: &[PickerToolId],
        ) -> std::result::Result<Vec<(ToolDescriptor, ToolDescriptor, f32)>, String> {
            Ok(vec![(
                descriptor(ids[0].name()),
                descriptor(ids[1].name()),
                0.97,
            )])
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

        async fn call(&self, _arguments: Value) -> Result<String> {
            Ok(String::new())
        }
    }

    fn callback_error(source: &FixtureSource, tools: &[Arc<dyn Tool>], code: &str) -> Error {
        let resolver = PickerResolver::new(source);
        let registry = ToolRegistry::new(tools.iter().map(AsRef::as_ref));
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
        let duplicate = CachedDecision::Duplicate(vec![descriptor("first"), descriptor("second")])
            .result("duplicate")
            .expect_err("duplicate must fail");
        assert!(matches!(
            duplicate,
            Error::Duplicate { capability, candidates }
                if capability == "duplicate"
                    && candidates == [
                        ToolId::new("tests", "first"),
                        ToolId::new("tests", "second")
                    ]
        ));
        assert!(matches!(
            CachedDecision::Absent.result("absent"),
            Err(Error::Absent { capability }) if capability == "absent"
        ));
        assert!(matches!(
            CachedDecision::Ambiguous(vec![descriptor("first"), descriptor("second")])
                .result("ambiguous"),
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
                if alias == "missing" && id == ToolId::new("tests", "first")
        ));
    }

    #[test]
    fn live_callbacks_reject_duplicate_aliases_and_identities() {
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FixtureTool {
            id: ToolId::new("tests", "first"),
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
                if id == ToolId::new("tests", "first")
                    && first_alias == "one"
                    && second_alias == "two"
        ));
    }

    #[test]
    fn runtime_rejects_duplicate_live_registry_ids() {
        let picker =
            ToolPicker::build(Catalog::default(), Config::default()).expect("empty picker builds");
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(FixtureTool {
                id: ToolId::new("tests", "same"),
            }),
            Arc::new(FixtureTool {
                id: ToolId::new("tests", "same"),
            }),
        ];
        let registry = ToolRegistry::new(tools.iter().map(AsRef::as_ref));
        assert!(matches!(
            RuntimeResolution::new(&picker, &registry, &ModelCatalog::empty()),
            Err(Error::DuplicateLiveToolId { id }) if id == ToolId::new("tests", "same")
        ));
    }

    #[test]
    fn diagnostics_are_identity_ordered_and_near_duplicates_are_forwarded() {
        let resolver = PickerResolver::new(&FixtureSource);
        assert_eq!(
            resolver.resolve("second").expect("second resolves"),
            ToolId::new("tests", "second")
        );
        assert_eq!(
            resolver.resolve("first").expect("first resolves"),
            ToolId::new("tests", "first")
        );
        let keys = resolver
            .diagnostics()
            .expect("diagnostics remain available")
            .into_keys()
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                ToolId::new("tests", "first"),
                ToolId::new("tests", "second")
            ]
        );

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
}
