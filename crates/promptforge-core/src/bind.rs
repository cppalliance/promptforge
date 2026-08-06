//! Synchronous prompt-level capability binding.
//!
//! [`bind_prompt`] executes the parsed H1 shared program once in Lua tool
//! declaration mode. Exact capability strings are resolved through the concrete
//! picker at most once during that pass. Binding then validates one-to-one
//! alias and identity mappings against the complete live registry. The
//! resulting [`BoundPrompt`] owns the parsed prompt, frozen declaration replay
//! data, atomic forward and reverse maps, and selected picker diagnostics, but
//! exposes no mutation path.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use promptforge_tool_picker::{Outcome, ToolDescriptor, ToolId as PickerToolId, ToolPicker};

use crate::lua::{ToolBindings, ToolResolver, bind_tool_declarations};
use crate::observe::{Observer, detail};
use crate::parser::Prompt;
use crate::tools::{ToolId, ToolRegistry};
use crate::{Error, Result};

/// A parsed prompt with one frozen H1 capability-binding result.
///
/// The original prompt, exact Lua declaration replay sequence, and selected
/// picker descriptors and validated forward and reverse maps are owned
/// together. All fields are private and every accessor is shared, so a caller
/// cannot change what later section VMs replay or dispatch.
#[derive(Debug, Clone)]
pub struct BoundPrompt {
    prompt: Prompt,
    bindings: ToolBindings,
    diagnostics: BTreeMap<ToolId, ToolDescriptor>,
    alias_to_id: BTreeMap<String, ToolId>,
    id_to_alias: BTreeMap<ToolId, String>,
}

impl BoundPrompt {
    #[cfg(test)]
    pub(crate) fn without_tools(prompt: Prompt) -> Self {
        Self {
            prompt,
            bindings: ToolBindings::default(),
            diagnostics: BTreeMap::new(),
            alias_to_id: BTreeMap::new(),
            id_to_alias: BTreeMap::new(),
        }
    }

    /// Returns the parsed prompt whose H1 declarations were bound.
    #[must_use]
    pub fn prompt(&self) -> &Prompt {
        &self.prompt
    }

    /// Returns the frozen bindings and exact declaration replay sequence.
    #[must_use]
    pub fn bindings(&self) -> &ToolBindings {
        &self.bindings
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
}

/// Binds every H1 `tools.need` declaration and validates the live registry.
///
/// The optional shared program is executed exactly once in the existing Lua
/// declaration mode. Repeated byte-identical capability descriptions replay a
/// cached picker decision, while descriptions differing by any byte are
/// resolved independently. The complete `registry` is then checked for repeated
/// stable identities before atomic forward and reverse binding maps are built.
/// Every selected identity must be present in that registry.
///
/// Binding observations are the fixed payload-free details from
/// [`crate::observe::detail`]. They contain no alias, capability, candidate,
/// identity, or picker diagnostic.
///
/// # Errors
/// Returns [`Error::Bind`] if the picker itself fails, [`Error::Absent`] when
/// nothing fits, [`Error::Duplicate`] for one server's duplicate matches,
/// [`Error::Ambiguous`] for a non-unique shortlist, or the existing Lua binding
/// errors for invalid declaration programs. Returns
/// [`Error::DuplicateLiveToolId`], [`Error::DuplicateAlias`],
/// [`Error::ToolIdSelectedTwice`], or [`Error::PickedToolNotLive`] when an
/// identity boundary is not one-to-one and complete.
pub fn bind_prompt(
    prompt: Prompt,
    picker: &ToolPicker,
    registry: &ToolRegistry<'_>,
    observer: &dyn Observer,
) -> Result<BoundPrompt> {
    bind_with_source(prompt, picker, registry, observer)
}

fn bind_with_source<S>(
    prompt: Prompt,
    source: &S,
    registry: &ToolRegistry<'_>,
    observer: &dyn Observer,
) -> Result<BoundPrompt>
where
    S: DecisionSource + ?Sized,
{
    let resolver = PickerResolver::new(source);
    let bindings = if let Some(shared) = &prompt.shared {
        bind_tool_declarations(shared, &resolver, observer, &prompt.title)?
    } else {
        observer.observe(&prompt.title, detail::TOOL_BINDING_STARTED);
        observer.observe(&prompt.title, detail::TOOL_BINDING_SUCCEEDED);
        ToolBindings::default()
    };
    let diagnostics = resolver.into_diagnostics()?;
    let (alias_to_id, id_to_alias) =
        validate_registry_and_bindings(&bindings, registry, observer, &prompt.title)?;

    Ok(BoundPrompt {
        prompt,
        bindings,
        diagnostics,
        alias_to_id,
        id_to_alias,
    })
}

fn validate_registry_and_bindings(
    bindings: &ToolBindings,
    registry: &ToolRegistry<'_>,
    observer: &dyn Observer,
    section: &str,
) -> Result<(BTreeMap<String, ToolId>, BTreeMap<ToolId, String>)> {
    observer.observe(section, detail::TOOL_REGISTRY_VALIDATION_STARTED);
    let result = validate_registry_and_bindings_inner(bindings, registry);
    observer.observe(
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
}

impl DecisionSource for ToolPicker {
    fn decide(&self, capability: &str) -> CachedDecision {
        CachedDecision::from_picker(self.resolve(capability))
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::{Value, json};

    use super::*;
    use crate::lua::LuaProgram;
    use crate::observe::NullObserver;
    use crate::observe::Observer;
    use crate::parser::Frontmatter;
    use crate::tools::Tool;

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
    struct Recorder(Mutex<Vec<(String, String)>>);

    impl Observer for Recorder {
        fn observe(&self, section: &str, detail: &str) {
            self.0
                .lock()
                .expect("fixture recorder must not be poisoned")
                .push((section.to_owned(), detail.to_owned()));
        }
    }

    impl Recorder {
        fn observations(&self) -> Vec<(String, String)> {
            self.0
                .lock()
                .expect("fixture recorder must not be poisoned")
                .clone()
        }
    }

    fn program(source: &str) -> LuaProgram {
        LuaProgram::compile(source, "shared", &NullObserver, "Prompt")
            .expect("fixture Lua must compile")
    }

    fn prompt(shared: Option<LuaProgram>) -> Prompt {
        Prompt {
            frontmatter: Frontmatter {
                name: "fixture".to_owned(),
                description: "fixture".to_owned(),
                version: 1,
                promptforge: Some(1),
                default_return: None,
                max_tool_iterations: None,
            },
            title: "Private title".to_owned(),
            shared,
            description_text: String::new(),
            sections: Vec::new(),
        }
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
        let bindings = bind_tool_declarations(&shared, &resolver, &NullObserver, "Prompt").unwrap();
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
                &recorder,
            );
            assert_eq!(result.is_ok(), outcome == "succeeded");
            let expected = if outcome == "succeeded" {
                vec![
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
                        detail::TOOL_REGISTRY_VALIDATION_STARTED.to_owned(),
                    ),
                    (
                        "Private title".to_owned(),
                        detail::TOOL_REGISTRY_VALIDATION_SUCCEEDED.to_owned(),
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
                        detail::TOOL_BINDING_FAILED.to_owned(),
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
        let bound = bind_with_source(prompt(None), &source, &registry, &recorder).unwrap();

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
                    detail::TOOL_REGISTRY_VALIDATION_STARTED.to_owned(),
                ),
                (
                    "Private title".to_owned(),
                    detail::TOOL_REGISTRY_VALIDATION_SUCCEEDED.to_owned(),
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
                    detail::TOOL_BINDING_SUCCEEDED.to_owned(),
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
