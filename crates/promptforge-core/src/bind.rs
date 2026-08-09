//! Synchronous prompt-level capability binding.
//!
//! [`bind_prompt`] executes the parsed H1 shared program once in Lua
//! declaration mode for tools and models. Exact capability strings are resolved
//! through the concrete pickers during that pass. Binding then validates
//! mappings against the live tool registry and model catalog. The resulting
//! [`BoundPrompt`] owns the parsed prompt, frozen declaration replay data, and
//! selected-set near-duplicate analysis for tools, but exposes no mutation path.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use mlua::{Lua, Scope};
use promptforge_tool_picker::{
    NearDuplicate, Outcome, ToolDescriptor, ToolId as PickerToolId, ToolPicker,
};

use crate::lua::{LiveBindingProducer, ToolBindings, ToolResolver, bind_shared_declarations};
use crate::model::{
    ModelBindings, ModelCatalog, ModelNeedOpts, ModelRegistry, ModelResolver, PickerModelResolver,
    ResolvedModel, model_picker_from,
};
use crate::observe::{Observer, detail};
use crate::parser::Prompt;
use crate::tools::{ToolId, ToolRegistry};
use crate::{Error, Result};

/// Run-scoped capability resolver and live H1 binding producer.
///
/// This is the dependency-safe runtime seam beside [`bind_prompt`]. It owns the
/// model picker rebuilt for this run, resolves each capability when Lua executes
/// the corresponding call, and accumulates frozen bindings for later section
/// VMs.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the live H1 runtime seam is wired into execution in step 3"
    )
)]
pub(crate) struct RuntimeResolution<'a, 'tools: 'a> {
    tool_resolver: PickerResolver<'a, ToolPicker>,
    registry: &'a ToolRegistry<'tools>,
    models: &'a ModelCatalog,
    model_picker: Option<ToolPicker>,
    producer: LiveBindingProducer,
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the live H1 runtime seam is wired into execution in step 3"
    )
)]
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

/// A parsed prompt with one frozen H1 capability-binding result.
///
/// The original prompt, exact Lua declaration replay sequence, and selected
/// picker descriptors, validated forward and reverse maps, and selected-set
/// near-duplicate analysis are owned together. All fields are private and every
/// accessor is shared, so a caller cannot change what later section VMs replay,
/// validate, or dispatch.
#[derive(Debug, Clone)]
pub struct BoundPrompt {
    prompt: Prompt,
    bindings: ToolBindings,
    models: ModelBindings,
    diagnostics: BTreeMap<ToolId, ToolDescriptor>,
    alias_to_id: BTreeMap<String, ToolId>,
    id_to_alias: BTreeMap<ToolId, String>,
    near_duplicates: Vec<NearDuplicate>,
}

impl BoundPrompt {
    #[cfg(test)]
    pub(crate) fn without_tools(prompt: Prompt) -> Self {
        Self {
            prompt,
            bindings: ToolBindings::default(),
            models: ModelBindings::default(),
            diagnostics: BTreeMap::new(),
            alias_to_id: BTreeMap::new(),
            id_to_alias: BTreeMap::new(),
            near_duplicates: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_tools(
        prompt: Prompt,
        bindings: ToolBindings,
        diagnostics: BTreeMap<ToolId, ToolDescriptor>,
        alias_to_id: BTreeMap<String, ToolId>,
        near_duplicates: Vec<NearDuplicate>,
        models: ModelBindings,
    ) -> Self {
        let id_to_alias = alias_to_id
            .iter()
            .map(|(alias, id)| (id.clone(), alias.clone()))
            .collect();
        Self {
            prompt,
            bindings,
            models,
            diagnostics,
            alias_to_id,
            id_to_alias,
            near_duplicates,
        }
    }

    /// Returns the parsed prompt whose H1 declarations were bound.
    #[must_use]
    pub fn prompt(&self) -> &Prompt {
        &self.prompt
    }

    /// Returns the frozen tool bindings and exact declaration replay sequence.
    #[must_use]
    pub fn bindings(&self) -> &ToolBindings {
        &self.bindings
    }

    /// Returns the frozen model bindings and exact declaration replay sequence.
    #[must_use]
    pub fn models(&self) -> &ModelBindings {
        &self.models
    }

    /// Returns selected picker descriptors keyed by stable core identity.
    #[must_use]
    pub fn diagnostics(&self) -> &BTreeMap<ToolId, ToolDescriptor> {
        &self.diagnostics
    }

    /// Returns the complete prompt-local alias to stable identity map.
    #[must_use]
    pub fn alias_to_id(&self) -> &BTreeMap<String, ToolId> {
        &self.alias_to_id
    }

    /// Returns the complete stable identity to prompt-local alias map.
    #[must_use]
    pub fn id_to_alias(&self) -> &BTreeMap<ToolId, String> {
        &self.id_to_alias
    }

    pub(crate) fn near_duplicates(&self) -> &[NearDuplicate] {
        &self.near_duplicates
    }
}

/// Binds every H1 `tools.need` and `models.need` declaration.
///
/// The optional shared program is executed exactly once. Tool capabilities
/// resolve through `picker`; model capabilities filter `models` then resolve
/// through a picker rebuilt from that catalog (reusing `picker`'s embedder).
/// Prompts that declare no `models.need` keep working with an empty catalog.
///
/// # Errors
/// Returns tool-binding errors as today, plus [`Error::ModelBind`],
/// [`Error::ModelAbsent`], [`Error::ModelDuplicate`], [`Error::ModelAmbiguous`],
/// [`Error::DuplicateModelAlias`], or [`Error::PickedModelNotLive`] for model
/// declarations. Same-weight model aliases with different invocation params are
/// legal and are not rejected as identity collisions.
pub fn bind_prompt(
    prompt: Prompt,
    picker: &ToolPicker,
    registry: &ToolRegistry<'_>,
    models: &ModelCatalog,
    execution: &str,
    observer: &dyn Observer,
) -> Result<BoundPrompt> {
    let model_picker = if models.is_empty() {
        None
    } else {
        Some(model_picker_from(picker, models)?)
    };
    bind_with_source(
        prompt,
        picker,
        registry,
        models,
        model_picker.as_ref(),
        execution,
        observer,
    )
}

fn bind_with_source<S>(
    prompt: Prompt,
    source: &S,
    registry: &ToolRegistry<'_>,
    models: &ModelCatalog,
    model_picker: Option<&ToolPicker>,
    execution: &str,
    observer: &dyn Observer,
) -> Result<BoundPrompt>
where
    S: DecisionSource + ?Sized,
{
    let resolver = PickerResolver::new(source);
    let model_resolver: Box<dyn ModelResolver> = match model_picker {
        Some(model_picker) => Box::new(PickerModelResolver::new(models, model_picker)),
        None => Box::new(
            |description: &str, _: &ModelNeedOpts| -> Result<ResolvedModel> {
                Err(Error::ModelAbsent {
                    capability: description.to_owned(),
                })
            },
        ),
    };
    let (bindings, model_bindings) = if let Some(shared) = &prompt.replay {
        bind_shared_declarations(
            shared,
            &resolver,
            model_resolver.as_ref(),
            execution,
            observer,
            &prompt.title,
        )?
    } else {
        observer.observe(execution, &prompt.title, detail::TOOL_BINDING_STARTED);
        observer.observe(execution, &prompt.title, detail::TOOL_BINDING_SUCCEEDED);
        observer.observe(execution, &prompt.title, detail::MODEL_BINDING_STARTED);
        observer.observe(execution, &prompt.title, detail::MODEL_BINDING_SUCCEEDED);
        (ToolBindings::default(), ModelBindings::default())
    };
    let diagnostics = resolver.into_diagnostics()?;
    let (alias_to_id, id_to_alias) =
        validate_registry_and_bindings(&bindings, registry, execution, observer, &prompt.title)?;
    validate_model_catalog(&model_bindings, models, execution, observer, &prompt.title)?;
    let selected_ids = bindings
        .bindings()
        .iter()
        .map(|binding| picker_id(binding.id()))
        .collect::<Vec<_>>();
    let near_duplicates = source
        .near_duplicates(&selected_ids)
        .map_err(|detail| Error::ToolScopeAnalysis { detail })?;

    Ok(BoundPrompt {
        prompt,
        bindings,
        models: model_bindings,
        diagnostics,
        alias_to_id,
        id_to_alias,
        near_duplicates,
    })
}

fn validate_model_catalog(
    bindings: &ModelBindings,
    catalog: &ModelCatalog,
    execution: &str,
    observer: &dyn Observer,
    section: &str,
) -> Result<()> {
    observer.observe(execution, section, detail::MODEL_CATALOG_VALIDATION_STARTED);
    let registry = ModelRegistry::new(catalog);
    let result = (|| {
        let mut seen_aliases = BTreeSet::new();
        for binding in bindings.bindings() {
            let alias = binding.alias().to_owned();
            if !seen_aliases.insert(alias.clone()) {
                return Err(Error::DuplicateModelAlias { alias });
            }
            if !registry.contains(binding.id()) {
                return Err(Error::PickedModelNotLive {
                    alias,
                    id: binding.id().clone(),
                });
            }
        }
        Ok(())
    })();
    observer.observe(
        execution,
        section,
        if result.is_ok() {
            detail::MODEL_CATALOG_VALIDATION_SUCCEEDED
        } else {
            detail::MODEL_CATALOG_VALIDATION_FAILED
        },
    );
    result
}

fn validate_registry_and_bindings(
    bindings: &ToolBindings,
    registry: &ToolRegistry<'_>,
    execution: &str,
    observer: &dyn Observer,
    section: &str,
) -> Result<(BTreeMap<String, ToolId>, BTreeMap<ToolId, String>)> {
    observer.observe(execution, section, detail::TOOL_REGISTRY_VALIDATION_STARTED);
    let result = validate_registry_and_bindings_inner(bindings, registry);
    observer.observe(
        execution,
        section,
        if result.is_ok() {
            detail::TOOL_REGISTRY_VALIDATION_SUCCEEDED
        } else {
            detail::TOOL_REGISTRY_VALIDATION_FAILED
        },
    );
    result
}

fn validate_registry_and_bindings_inner(
    bindings: &ToolBindings,
    registry: &ToolRegistry<'_>,
) -> Result<(BTreeMap<String, ToolId>, BTreeMap<ToolId, String>)> {
    let mut live_ids = BTreeSet::new();
    for tool in registry.tools() {
        let id = tool.id();
        if !live_ids.insert(id.clone()) {
            return Err(Error::DuplicateLiveToolId { id });
        }
    }

    let mut alias_to_id = BTreeMap::new();
    let mut id_to_alias = BTreeMap::new();
    for binding in bindings.bindings() {
        let alias = binding.alias().to_owned();
        let id = binding.id().clone();
        if alias_to_id.insert(alias.clone(), id.clone()).is_some() {
            return Err(Error::DuplicateAlias { alias });
        }
        if let Some(first_alias) = id_to_alias.insert(id.clone(), alias.clone()) {
            return Err(Error::ToolIdSelectedTwice {
                id,
                first_alias,
                second_alias: alias,
            });
        }
        if !live_ids.contains(&id) {
            return Err(Error::PickedToolNotLive { alias, id });
        }
    }

    Ok((alias_to_id, id_to_alias))
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
    fn from_picker(outcome: std::result::Result<Outcome, promptforge_tool_picker::Error>) -> Self {
        match outcome {
            Ok(Outcome::Bind(tool)) => Self::Bind(tool),
            Ok(Outcome::Absent) => Self::Absent,
            Ok(Outcome::Duplicate(tools)) => Self::Duplicate(tools),
            Ok(Outcome::Ambiguous(tools)) => Self::Ambiguous(tools),
            Err(error) => Self::Failed(error.to_string()),
        }
    }

    fn result(&self, capability: &str) -> Result<ToolId> {
        match self {
            Self::Bind(tool) => Ok(core_id(&tool.id)),
            Self::Absent => Err(Error::Absent {
                capability: capability.to_owned(),
            }),
            Self::Duplicate(tools) => Err(Error::Duplicate {
                capability: capability.to_owned(),
                candidates: tools.iter().map(|tool| core_id(&tool.id)).collect(),
            }),
            Self::Ambiguous(tools) => Err(Error::Ambiguous {
                capability: capability.to_owned(),
                candidates: tools.iter().map(|tool| core_id(&tool.id)).collect(),
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

    fn near_duplicates(
        &self,
        ids: &[PickerToolId],
    ) -> std::result::Result<Vec<NearDuplicate>, String>;
}

impl DecisionSource for ToolPicker {
    fn decide(&self, capability: &str) -> CachedDecision {
        CachedDecision::from_picker(self.resolve(capability))
    }

    fn near_duplicates(
        &self,
        ids: &[PickerToolId],
    ) -> std::result::Result<Vec<NearDuplicate>, String> {
        ToolPicker::near_duplicates(self, ids).map_err(|error| error.to_string())
    }
}

#[derive(Debug)]
struct ResolverState {
    replay: BTreeMap<String, CachedDecision>,
    diagnostics: BTreeMap<ToolId, ToolDescriptor>,
}

impl ResolverState {
    fn new() -> Self {
        Self {
            replay: BTreeMap::new(),
            diagnostics: BTreeMap::new(),
        }
    }
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
            state: Mutex::new(ResolverState::new()),
        }
    }

    fn into_diagnostics(self) -> Result<BTreeMap<ToolId, ToolDescriptor>> {
        self.state
            .into_inner()
            .map(|state| state.diagnostics)
            .map_err(|_| Error::Lua("tool picker binding cache was poisoned".to_owned()))
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
            .map_err(|_| Error::Lua("tool picker binding cache was poisoned".to_owned()))?;
        let decision = state
            .replay
            .entry(capability.to_owned())
            .or_insert_with(|| self.source.decide(capability))
            .clone();
        if let CachedDecision::Bind(tool) = &decision {
            state.diagnostics.insert(core_id(&tool.id), tool.clone());
        }
        decision.result(capability)
    }
}

fn core_id(id: &PickerToolId) -> ToolId {
    ToolId::new(id.server(), id.name())
}

fn picker_id(id: &ToolId) -> PickerToolId {
    PickerToolId::new(id.server(), id.name())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::{Value, json};

    use promptforge_tool_picker::{Catalog, Config as PickerConfig};

    use super::*;
    use crate::lua::{LuaProgram, bind_tool_declarations};
    use crate::model::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
    use crate::observe::NullObserver;
    use crate::observe::Observer;
    use crate::parser::Frontmatter;
    use crate::tools::Tool;

    const EXECUTION: &str = "bind-test";

    #[derive(Debug)]
    struct FixtureSource {
        calls: AtomicUsize,
    }

    impl FixtureSource {
        fn tool(server: &str, name: &str) -> ToolDescriptor {
            ToolDescriptor::new(
                PickerToolId::new(server, name),
                "diagnostic prose",
                json!({}),
            )
        }
    }

    impl DecisionSource for FixtureSource {
        fn decide(&self, capability: &str) -> CachedDecision {
            self.calls.fetch_add(1, Ordering::Relaxed);
            match capability {
                "bind" | "private selected capability" => {
                    CachedDecision::Bind(Self::tool("server", "bound"))
                }
                "other" => CachedDecision::Bind(Self::tool("server", "other")),
                "absent" | "private missing capability" => CachedDecision::Absent,
                "duplicate" => CachedDecision::Duplicate(vec![
                    Self::tool("server", "one"),
                    Self::tool("server", "two"),
                ]),
                "ambiguous" => CachedDecision::Ambiguous(vec![
                    Self::tool("one", "tool"),
                    Self::tool("two", "tool"),
                ]),
                _ => CachedDecision::Failed("fixture picker failure".to_owned()),
            }
        }

        fn near_duplicates(
            &self,
            _ids: &[PickerToolId],
        ) -> std::result::Result<Vec<NearDuplicate>, String> {
            Ok(Vec::new())
        }
    }

    #[derive(Debug)]
    struct AnalysisSource {
        fail: bool,
    }

    impl DecisionSource for AnalysisSource {
        fn decide(&self, capability: &str) -> CachedDecision {
            CachedDecision::Bind(FixtureSource::tool("server", capability))
        }

        fn near_duplicates(
            &self,
            ids: &[PickerToolId],
        ) -> std::result::Result<Vec<NearDuplicate>, String> {
            if self.fail {
                return Err("private analysis failure".to_owned());
            }
            Ok(vec![NearDuplicate {
                first: FixtureSource::tool(ids[0].server(), ids[0].name()),
                second: FixtureSource::tool(ids[1].server(), ids[1].name()),
                similarity: 0.96,
            }])
        }
    }

    #[derive(Debug)]
    struct FixtureLiveTool {
        id: ToolId,
    }

    impl FixtureLiveTool {
        fn new(server: &str, name: &str) -> Self {
            Self {
                id: ToolId::new(server, name),
            }
        }
    }

    #[async_trait::async_trait]
    impl Tool for FixtureLiveTool {
        fn id(&self) -> ToolId {
            self.id.clone()
        }

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn wire_name(&self) -> &str {
            "fixture"
        }

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn description(&self) -> &str {
            "fixture"
        }

        fn parameters_schema(&self) -> Value {
            json!({"type": "object"})
        }

        async fn call(&self, _args: Value) -> Result<String> {
            Ok(String::new())
        }
    }

    #[derive(Debug, Default)]
    struct Recorder(Mutex<Vec<(String, String, String)>>);

    impl Observer for Recorder {
        fn observe(&self, execution: &str, section: &str, detail: &str) {
            self.0
                .lock()
                .expect("fixture recorder must not be poisoned")
                .push((execution.to_owned(), section.to_owned(), detail.to_owned()));
        }
    }

    impl Recorder {
        fn records(&self) -> Vec<(String, String, String)> {
            self.0
                .lock()
                .expect("fixture recorder must not be poisoned")
                .clone()
        }

        fn observations(&self) -> Vec<(String, String)> {
            self.0
                .lock()
                .expect("fixture recorder must not be poisoned")
                .iter()
                .map(|(_, section, detail)| (section.clone(), detail.clone()))
                .collect()
        }
    }

    fn program(source: &str) -> LuaProgram {
        LuaProgram::compile(source, "shared", 1, EXECUTION, &NullObserver, "Prompt")
            .expect("fixture Lua must compile")
    }

    fn prompt(replay: Option<LuaProgram>) -> Prompt {
        Prompt {
            frontmatter: Frontmatter {
                name: "fixture".to_owned(),
                description: "fixture".to_owned(),
                promptforge: Some(1),
                default_return: None,
                max_tool_iterations: None,
            },
            title: "Private title".to_owned(),
            replay,
            h1_blocks: Vec::new(),
            description_text: String::new(),
            sections: Vec::new(),
        }
    }

    #[test]
    fn runtime_resolution_resolves_only_executed_h1_calls() {
        let live = FixtureLiveTool::new("server", "search");
        let registry = ToolRegistry::new([&live as &dyn Tool]);
        let picker = ToolPicker::build(
            Catalog::new(vec![ToolDescriptor::new(
                PickerToolId::new("server", "search"),
                "Search the web",
                json!({"type": "object"}),
            )]),
            PickerConfig::default(),
        )
        .expect("tool picker must build");
        let models = ModelCatalog::new([ModelDescriptor::new(
            ModelId::gateway("analyst"),
            "Careful analysis",
            131_072,
            ThinkingMode::Never,
        )]);
        let runtime = RuntimeResolution::new(&picker, &registry, &models)
            .expect("runtime resolver must build");
        let lua = Lua::new();

        lua.scope(|scope| {
            runtime
                .install(&lua, scope)
                .map_err(|error| mlua::Error::external(error.to_string()))?;
            lua.load(
                r#"
                if false then
                    tools.need("skipped", "Search the web")
                end
                local search = tools.need("search", "Search the web")
                assert(search.wire_name == "fixture")
                assert(search.parameters.type == "object")
                tools.always("search")
                local writer = models.always("writer", "Careful analysis")
                assert(writer.model_id == "analyst")
                "#,
            )
            .exec()
        })
        .expect("live H1 calls must resolve");

        assert!(runtime.take_callback_error().unwrap().is_none());
        let (tools, model_bindings) = runtime.bindings().expect("bindings must snapshot");
        assert_eq!(tools.bindings().len(), 1);
        assert_eq!(tools.bindings()[0].alias(), "search");
        assert_eq!(tools.always(), &["search"]);
        assert_eq!(model_bindings.bindings().len(), 1);
        assert_eq!(model_bindings.bindings()[0].alias(), "writer");
        assert_eq!(model_bindings.always(), Some("writer"));
    }

    #[test]
    fn runtime_resolution_preserves_typed_picker_errors() {
        let picker = ToolPicker::build(Catalog::default(), PickerConfig::default())
            .expect("empty tool picker must build");
        let registry = ToolRegistry::new(std::iter::empty());
        let models = ModelCatalog::empty();
        let runtime = RuntimeResolution::new(&picker, &registry, &models)
            .expect("runtime resolver must build");
        let lua = Lua::new();

        let error = lua
            .scope(|scope| {
                runtime
                    .install(&lua, scope)
                    .map_err(|error| mlua::Error::external(error.to_string()))?;
                lua.load(r#"tools.need("search", "Search the web")"#).exec()
            })
            .expect_err("missing capability must fail at the executed call");
        assert!(
            error
                .to_string()
                .contains("tool capability resolution failed")
        );
        assert!(matches!(
            runtime.take_callback_error().unwrap(),
            Some(Error::Absent { capability }) if capability == "Search the web"
        ));
        assert!(runtime.bindings().unwrap().0.bindings().is_empty());
    }

    #[test]
    fn exact_capability_replay_resolves_once_and_retains_diagnostics() {
        let source = FixtureSource {
            calls: AtomicUsize::new(0),
        };
        let resolver = PickerResolver::new(&source);
        let shared = program(
            "tools.need('first', 'bind')\n\
             tools.need('second', 'bind')",
        );
        let bindings =
            bind_tool_declarations(&shared, &resolver, EXECUTION, &NullObserver, "Prompt").unwrap();
        let diagnostics = resolver.into_diagnostics().unwrap();

        assert_eq!(source.calls.load(Ordering::Relaxed), 1);
        assert_eq!(bindings.bindings().len(), 2);
        assert_eq!(bindings.bindings()[0].id(), bindings.bindings()[1].id());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics
                .get(&ToolId::new("server", "bound"))
                .map(|tool| tool.description.as_str()),
            Some("diagnostic prose")
        );
    }

    #[test]
    fn cache_key_is_the_exact_unnormalized_capability() {
        let source = FixtureSource {
            calls: AtomicUsize::new(0),
        };
        let resolver = PickerResolver::new(&source);
        resolver.resolve("bind").unwrap();
        resolver.resolve("bind").unwrap();
        resolver.resolve("Bind").unwrap_err();
        assert_eq!(source.calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn concrete_bind_outcome_maps_to_core_tool_id() {
        let decision = CachedDecision::from_picker(Ok(Outcome::Bind(FixtureSource::tool(
            "selected-server",
            "selected-tool",
        ))));

        assert_eq!(
            decision.result("exact capability").unwrap(),
            ToolId::new("selected-server", "selected-tool")
        );
    }

    #[test]
    fn concrete_absent_outcome_maps_to_core_absent_error() {
        let decision = CachedDecision::from_picker(Ok(Outcome::Absent));

        assert!(matches!(
            decision.result("exact capability"),
            Err(Error::Absent { capability }) if capability == "exact capability"
        ));
    }

    #[test]
    fn concrete_duplicate_outcome_maps_to_ordered_core_ids() {
        let decision = CachedDecision::from_picker(Ok(Outcome::Duplicate(vec![
            FixtureSource::tool("second-server", "second-tool"),
            FixtureSource::tool("first-server", "first-tool"),
        ])));

        assert!(matches!(
            decision.result("exact capability"),
            Err(Error::Duplicate {
                capability,
                candidates,
            }) if capability == "exact capability"
                && candidates == [
                    ToolId::new("second-server", "second-tool"),
                    ToolId::new("first-server", "first-tool"),
                ]
        ));
    }

    #[test]
    fn concrete_ambiguous_outcome_maps_to_ordered_core_ids() {
        let decision = CachedDecision::from_picker(Ok(Outcome::Ambiguous(vec![
            FixtureSource::tool("z-server", "z-tool"),
            FixtureSource::tool("a-server", "a-tool"),
        ])));

        assert!(matches!(
            decision.result("exact capability"),
            Err(Error::Ambiguous {
                capability,
                candidates,
            }) if capability == "exact capability"
                && candidates == [
                    ToolId::new("z-server", "z-tool"),
                    ToolId::new("a-server", "a-tool"),
                ]
        ));
    }

    #[test]
    fn four_picker_results_map_to_distinct_core_errors() {
        let source = FixtureSource {
            calls: AtomicUsize::new(0),
        };
        let resolver = PickerResolver::new(&source);

        assert!(resolver.resolve("bind").is_ok());
        assert!(matches!(
            resolver.resolve("absent"),
            Err(Error::Absent { .. })
        ));
        assert!(matches!(
            resolver.resolve("duplicate"),
            Err(Error::Duplicate { candidates, .. }) if candidates.len() == 2
        ));
        assert!(matches!(
            resolver.resolve("ambiguous"),
            Err(Error::Ambiguous { candidates, .. }) if candidates.len() == 2
        ));
        assert!(matches!(
            resolver.resolve("failed"),
            Err(Error::Bind { .. })
        ));
    }

    #[test]
    fn structured_picker_outcomes_survive_the_lua_callback_boundary() {
        for (capability, expected) in [
            ("absent", "absent"),
            ("duplicate", "duplicate"),
            ("ambiguous", "ambiguous"),
            ("failed", "bind"),
        ] {
            let source = FixtureSource {
                calls: AtomicUsize::new(0),
            };
            let resolver = PickerResolver::new(&source);
            let error = bind_tool_declarations(
                &program(&format!("tools.need('alias', {capability:?})")),
                &resolver,
                EXECUTION,
                &NullObserver,
                "Prompt",
            )
            .unwrap_err();
            assert!(
                matches!(
                    (&error, expected),
                    (Error::Absent { .. }, "absent")
                        | (Error::Duplicate { .. }, "duplicate")
                        | (Error::Ambiguous { .. }, "ambiguous")
                        | (Error::Bind { .. }, "bind")
                ),
                "wrong structured error for {capability:?}: {error:?}"
            );
        }
    }

    #[test]
    fn resolver_failures_cannot_be_suppressed_with_lua_pcall() {
        let source = FixtureSource {
            calls: AtomicUsize::new(0),
        };
        let resolver = PickerResolver::new(&source);
        let error = bind_tool_declarations(
            &program("pcall(tools.need, 'alias', 'absent')"),
            &resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap_err();
        assert!(matches!(error, Error::Absent { .. }));
    }

    #[test]
    fn bound_prompt_is_frozen_and_retains_selected_diagnostics() {
        let source = FixtureSource {
            calls: AtomicUsize::new(0),
        };
        let live = FixtureLiveTool::new("server", "bound");
        let other = FixtureLiveTool::new("server", "other");
        let registry = ToolRegistry::new([&live as &dyn Tool, &other as &dyn Tool]);
        let bound = bind_with_source(
            prompt(Some(program(
                "tools.need('alias', 'bind')\ntools.need('other_alias', 'other')",
            ))),
            &source,
            &registry,
            &ModelCatalog::empty(),
            None,
            EXECUTION,
            &NullObserver,
        )
        .unwrap();
        assert_eq!(bound.prompt().title, "Private title");
        assert_eq!(bound.bindings().bindings()[0].alias(), "alias");
        assert_eq!(bound.diagnostics().len(), 2);
        assert_eq!(
            bound.alias_to_id().get("alias"),
            Some(&ToolId::new("server", "bound"))
        );
        assert_eq!(
            bound.alias_to_id().get("other_alias"),
            Some(&ToolId::new("server", "other"))
        );
        assert_eq!(
            bound.id_to_alias().get(&ToolId::new("server", "bound")),
            Some(&"alias".to_owned())
        );
        assert_eq!(
            bound.id_to_alias().get(&ToolId::new("server", "other")),
            Some(&"other_alias".to_owned())
        );
    }

    #[test]
    fn binding_precomputes_picker_near_duplicates_for_runtime_scope_validation() {
        let first = FixtureLiveTool::new("server", "first");
        let second = FixtureLiveTool::new("server", "second");
        let registry = ToolRegistry::new([&first as &dyn Tool, &second as &dyn Tool]);
        let bound = bind_with_source(
            prompt(Some(program(
                "tools.need('first_alias', 'first')\n\
                 tools.need('second_alias', 'second')",
            ))),
            &AnalysisSource { fail: false },
            &registry,
            &ModelCatalog::empty(),
            None,
            EXECUTION,
            &NullObserver,
        )
        .unwrap();

        assert_eq!(bound.near_duplicates().len(), 1);
        assert_eq!(bound.near_duplicates()[0].first.name(), "first");
        assert_eq!(bound.near_duplicates()[0].second.name(), "second");
        assert!((bound.near_duplicates()[0].similarity - 0.96).abs() < f32::EPSILON);
    }

    #[test]
    fn picker_scope_analysis_failure_prevents_a_bound_prompt() {
        let first = FixtureLiveTool::new("server", "first");
        let second = FixtureLiveTool::new("server", "second");
        let registry = ToolRegistry::new([&first as &dyn Tool, &second as &dyn Tool]);
        let error = bind_with_source(
            prompt(Some(program(
                "tools.need('first_alias', 'first')\n\
                 tools.need('second_alias', 'second')",
            ))),
            &AnalysisSource { fail: true },
            &registry,
            &ModelCatalog::empty(),
            None,
            EXECUTION,
            &NullObserver,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            Error::ToolScopeAnalysis { detail } if detail == "private analysis failure"
        ));
    }

    #[test]
    fn binding_reports_are_fixed_ordered_and_payload_free() {
        for (capability, outcome) in [
            ("private selected capability", "succeeded"),
            ("private missing capability", "failed"),
        ] {
            let source = FixtureSource {
                calls: AtomicUsize::new(0),
            };
            let live = FixtureLiveTool::new("server", "bound");
            let registry = ToolRegistry::new([&live as &dyn Tool]);
            let recorder = Recorder::default();
            let result = bind_with_source(
                prompt(Some(program(&format!(
                    "tools.need('private_alias', {capability:?})"
                )))),
                &source,
                &registry,
                &ModelCatalog::empty(),
                None,
                EXECUTION,
                &recorder,
            );
            assert_eq!(result.is_ok(), outcome == "succeeded");
            assert!(
                recorder
                    .records()
                    .iter()
                    .all(|(execution, _, _)| execution == EXECUTION)
            );
            let expected = if outcome == "succeeded" {
                vec![
                    (
                        "Private title".to_owned(),
                        detail::TOOL_BINDING_STARTED.to_owned(),
                    ),
                    (
                        "Private title".to_owned(),
                        detail::MODEL_BINDING_STARTED.to_owned(),
                    ),
                    (
                        "Private title".to_owned(),
                        detail::TOOL_BINDING_SUCCEEDED.to_owned(),
                    ),
                    (
                        "Private title".to_owned(),
                        detail::MODEL_BINDING_SUCCEEDED.to_owned(),
                    ),
                    (
                        "Private title".to_owned(),
                        detail::TOOL_REGISTRY_VALIDATION_STARTED.to_owned(),
                    ),
                    (
                        "Private title".to_owned(),
                        detail::TOOL_REGISTRY_VALIDATION_SUCCEEDED.to_owned(),
                    ),
                    (
                        "Private title".to_owned(),
                        detail::MODEL_CATALOG_VALIDATION_STARTED.to_owned(),
                    ),
                    (
                        "Private title".to_owned(),
                        detail::MODEL_CATALOG_VALIDATION_SUCCEEDED.to_owned(),
                    ),
                ]
            } else {
                vec![
                    (
                        "Private title".to_owned(),
                        detail::TOOL_BINDING_STARTED.to_owned(),
                    ),
                    (
                        "Private title".to_owned(),
                        detail::MODEL_BINDING_STARTED.to_owned(),
                    ),
                    (
                        "Private title".to_owned(),
                        detail::TOOL_BINDING_FAILED.to_owned(),
                    ),
                    (
                        "Private title".to_owned(),
                        detail::MODEL_BINDING_FAILED.to_owned(),
                    ),
                ]
            };
            assert_eq!(recorder.observations(), expected);
            let trace = format!("{:?}", recorder.observations());
            assert!(!trace.contains("private_alias"));
            assert!(!trace.contains(capability));
            assert!(!trace.contains("server"));
        }
    }

    #[test]
    fn prompt_without_shared_code_binds_to_an_empty_frozen_result() {
        let source = FixtureSource {
            calls: AtomicUsize::new(0),
        };
        let recorder = Recorder::default();
        let registry = ToolRegistry::new(std::iter::empty());
        let bound = bind_with_source(
            prompt(None),
            &source,
            &registry,
            &ModelCatalog::empty(),
            None,
            EXECUTION,
            &recorder,
        )
        .unwrap();

        assert!(bound.bindings().bindings().is_empty());
        assert!(bound.diagnostics().is_empty());
        assert_eq!(source.calls.load(Ordering::Relaxed), 0);
        assert_eq!(
            recorder.observations(),
            [
                (
                    "Private title".to_owned(),
                    detail::TOOL_BINDING_STARTED.to_owned(),
                ),
                (
                    "Private title".to_owned(),
                    detail::TOOL_BINDING_SUCCEEDED.to_owned(),
                ),
                (
                    "Private title".to_owned(),
                    detail::MODEL_BINDING_STARTED.to_owned(),
                ),
                (
                    "Private title".to_owned(),
                    detail::MODEL_BINDING_SUCCEEDED.to_owned(),
                ),
                (
                    "Private title".to_owned(),
                    detail::TOOL_REGISTRY_VALIDATION_STARTED.to_owned(),
                ),
                (
                    "Private title".to_owned(),
                    detail::TOOL_REGISTRY_VALIDATION_SUCCEEDED.to_owned(),
                ),
                (
                    "Private title".to_owned(),
                    detail::MODEL_CATALOG_VALIDATION_STARTED.to_owned(),
                ),
                (
                    "Private title".to_owned(),
                    detail::MODEL_CATALOG_VALIDATION_SUCCEEDED.to_owned(),
                ),
            ]
        );
    }

    #[test]
    fn duplicate_live_ids_fail_before_a_bound_prompt_exists() {
        let source = FixtureSource {
            calls: AtomicUsize::new(0),
        };
        let first = FixtureLiveTool::new("server", "bound");
        let repeated = FixtureLiveTool::new("server", "bound");
        let registry = ToolRegistry::new([&first as &dyn Tool, &repeated as &dyn Tool]);

        let error = bind_with_source(
            prompt(Some(program("tools.need('alias', 'bind')"))),
            &source,
            &registry,
            &ModelCatalog::empty(),
            None,
            EXECUTION,
            &NullObserver,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            Error::DuplicateLiveToolId { id } if id == ToolId::new("server", "bound")
        ));
    }

    #[test]
    fn two_aliases_selecting_one_live_id_fail_atomically() {
        let source = FixtureSource {
            calls: AtomicUsize::new(0),
        };
        let live = FixtureLiveTool::new("server", "bound");
        let registry = ToolRegistry::new([&live as &dyn Tool]);

        let error = bind_with_source(
            prompt(Some(program(
                "tools.need('first', 'bind')\ntools.need('second', 'bind')",
            ))),
            &source,
            &registry,
            &ModelCatalog::empty(),
            None,
            EXECUTION,
            &NullObserver,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            Error::ToolIdSelectedTwice {
                id,
                first_alias,
                second_alias,
            } if id == ToolId::new("server", "bound")
                && first_alias == "first"
                && second_alias == "second"
        ));
    }

    #[test]
    fn shipped_analyst_example_binds_against_gateway_shaped_catalog() {
        // Vendor model ids must not ride in the picker's embedding name, or
        // ids like claude-sonnet-4-6 drown the capability description.
        let catalog = ModelCatalog::new([ModelDescriptor::new(
            ModelId::gateway("claude-sonnet-4-6"),
            "A model suited for careful analysis, coding, and general assistance",
            200_000,
            ThinkingMode::Never,
        )]);
        let picker = ToolPicker::build(Catalog::default(), PickerConfig::default())
            .expect("empty tool picker must build");
        let registry = ToolRegistry::new(std::iter::empty());
        let source = include_str!("../../../prompts/analyst-example.md");
        let parsed =
            Prompt::parse(source, EXECUTION, &NullObserver).expect("parse analyst example");
        let bound = bind_prompt(
            parsed,
            &picker,
            &registry,
            &catalog,
            EXECUTION,
            &NullObserver,
        )
        .expect("parsed analyst example must bind");
        assert_eq!(bound.models().bindings()[0].alias(), "analyst");
        assert_eq!(
            bound.models().bindings()[0].id(),
            &ModelId::gateway("claude-sonnet-4-6")
        );
    }

    #[test]
    fn two_model_aliases_sharing_one_id_with_different_invocation_bind() {
        // Tools reject one live id under two aliases; models keep that legal
        // when the aliases freeze different invocation params.
        let catalog = ModelCatalog::new([ModelDescriptor::new(
            ModelId::gateway("analyst"),
            "A careful analysis model",
            131_072,
            ThinkingMode::Switchable,
        )]);
        let picker = ToolPicker::build(Catalog::default(), PickerConfig::default())
            .expect("empty tool picker must build");
        let registry = ToolRegistry::new(std::iter::empty());

        let bound = bind_prompt(
            prompt(Some(program(
                r#"models.need("cool", "careful analysis", { temperature = 0, thinking = false })
models.need("warm", "careful analysis", { temperature = 0.7, thinking = true, max_tokens = 128 })"#,
            ))),
            &picker,
            &registry,
            &catalog,
            EXECUTION,
            &NullObserver,
        )
        .expect("same ModelId under two aliases with different invocation must bind");

        let models = bound.models().bindings();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].alias(), "cool");
        assert_eq!(models[1].alias(), "warm");
        assert_eq!(models[0].id(), models[1].id());
        assert_eq!(models[0].id().name(), "analyst");
        assert_ne!(models[0].invocation(), models[1].invocation());
        assert_eq!(models[0].invocation().temperature, Some(0.0));
        assert_eq!(models[0].invocation().thinking, Some(false));
        assert_eq!(models[1].invocation().temperature, Some(0.7));
        assert_eq!(models[1].invocation().thinking, Some(true));
        assert_eq!(models[1].invocation().max_tokens, Some(128));
    }

    #[test]
    fn picker_selected_id_absent_from_live_registry_fails_atomically() {
        let source = FixtureSource {
            calls: AtomicUsize::new(0),
        };
        let unrelated = FixtureLiveTool::new("server", "other");
        let registry = ToolRegistry::new([&unrelated as &dyn Tool]);

        let error = bind_with_source(
            prompt(Some(program("tools.need('alias', 'bind')"))),
            &source,
            &registry,
            &ModelCatalog::empty(),
            None,
            EXECUTION,
            &NullObserver,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            Error::PickedToolNotLive { alias, id }
                if alias == "alias" && id == ToolId::new("server", "bound")
        ));
    }

    #[test]
    fn registry_validation_failure_reports_are_ordered_and_payload_free() {
        let source = FixtureSource {
            calls: AtomicUsize::new(0),
        };
        let recorder = Recorder::default();
        let registry = ToolRegistry::new(std::iter::empty());

        let result = bind_with_source(
            prompt(Some(program(
                "tools.need('private_alias', 'private selected capability')",
            ))),
            &source,
            &registry,
            &ModelCatalog::empty(),
            None,
            EXECUTION,
            &recorder,
        );

        assert!(matches!(result, Err(Error::PickedToolNotLive { .. })));
        assert_eq!(
            recorder.observations(),
            [
                (
                    "Private title".to_owned(),
                    detail::TOOL_BINDING_STARTED.to_owned(),
                ),
                (
                    "Private title".to_owned(),
                    detail::MODEL_BINDING_STARTED.to_owned(),
                ),
                (
                    "Private title".to_owned(),
                    detail::TOOL_BINDING_SUCCEEDED.to_owned(),
                ),
                (
                    "Private title".to_owned(),
                    detail::MODEL_BINDING_SUCCEEDED.to_owned(),
                ),
                (
                    "Private title".to_owned(),
                    detail::TOOL_REGISTRY_VALIDATION_STARTED.to_owned(),
                ),
                (
                    "Private title".to_owned(),
                    detail::TOOL_REGISTRY_VALIDATION_FAILED.to_owned(),
                ),
            ]
        );
        let trace = format!("{:?}", recorder.observations());
        assert!(!trace.contains("private_alias"));
        assert!(!trace.contains("private selected capability"));
        assert!(!trace.contains("server"));
        assert!(!trace.contains("bound"));
    }
}
