//! The execute subtree's ambient run state.
//!
//! [`RunContext`] is built once in [`run`](super::run) and travels through
//! the execute subtree as parameter one (`ctx: &RunContext`). The
//! invariant: a new run-scoped concern becomes a field here, never a new
//! parameter. Per-call data (a section, a reply, a `var` snapshot) stays
//! in parameters or on the per-section frame.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64};

use crate::Result;
use crate::client::GatewayClient;
use crate::debug::DebugCapture;
use crate::lua::{LuaProgram, SectionVm, ToolBindings};
use crate::model::ModelBindings;
use crate::observe::Observer;
use crate::parser::Prompt;
use crate::store::{StoreRef, WriteScope};
use crate::tools::SharedTools;
use crate::untrusted::GuardNonce;

use super::config::{RunConfig, RunLimits};
use super::gateway::GatewaySource;
use super::scope::ToolAnalysis;
use super::section_vm::{SectionVmSetup, VmSeed};
use super::support::{now_rfc3339_checked, sys_json};

/// The ambient state one run shares across the execute subtree.
///
/// Immutable for the run's lifetime and cheap to clone: every field is
/// shared ownership or `Copy`, so a clone points at the same run state.
/// The three sanctioned forks: [`with_walk_state`](Self::with_walk_state)
/// at the H1-to-walk handoff,
/// [`with_effective_handles`](Self::with_effective_handles) for a fanout's
/// proxy reporting handles, and [`with_args`](Self::with_args) carrying an
/// `execute` call's args override into its contained chain.
#[derive(Clone)]
pub(crate) struct RunContext {
    /// The prompt this run executes.
    prompt: Arc<Prompt>,
    /// The untrusted-envelope nonce, minted once here so every wrap in the
    /// run shares it.
    nonce: GuardNonce,
    /// The run-scoped store backing every section's Lua `store` table.
    store: StoreRef,
    /// The execution identifier every observation carries.
    execution: Arc<str>,
    /// The run's argument string for `{{ args }}` substitution.
    args: Arc<str>,
    /// The run's resource limits.
    limits: RunLimits,
    /// The run's observer handle.
    observer: Arc<dyn Observer>,
    /// Opt-in raw request/response capture for each model turn.
    debug: Option<Arc<dyn DebugCapture>>,
    /// The model-turn counter this context advances (the run's, or one
    /// shared by all arms of a fanout).
    turns: Arc<AtomicU32>,
    /// The run-global execution-id counter: every section entry and every
    /// fanout arm takes the next value (H1 keeps id 0). A fanout shares it
    /// without resetting, unlike `turns`.
    ids: Arc<AtomicU64>,
    /// The shared library replayed as every section's first chunk; an empty
    /// compiled chunk when the prompt declares no `lua shared` library, so
    /// the startup sequence carries no `Option` branch.
    shared: Arc<LuaProgram>,
    /// The run's shared tool registry handle.
    shared_tools: SharedTools,
    /// The frozen prompt-level tool bindings; empty until the walk starts
    /// (the live H1 pass resolves them and never reads them back).
    bindings: Arc<ToolBindings>,
    /// The frozen prompt-level model bindings; empty until the walk starts.
    models: Arc<ModelBindings>,
    /// The prompt's tool-scope analysis (semantic duplicate check,
    /// aliases); empty until the walk starts.
    analysis: Arc<ToolAnalysis>,
    /// The walk's start timestamp, stamped into every section's `sys.when`;
    /// empty until the walk starts (H1 stamps its own `now`).
    when: Arc<str>,
}

impl RunContext {
    /// Builds the context for one run of `prompt`. The turn and id counters
    /// are minted here (both start at zero); the walk-scoped fields
    /// (`bindings`, `models`, `analysis`, `when`) start empty and take their
    /// live values at the H1-to-walk handoff.
    #[must_use]
    pub(crate) fn new(
        prompt: &Prompt,
        args: &str,
        store: &StoreRef,
        shared_tools: SharedTools,
        shared: LuaProgram,
        config: &RunConfig,
    ) -> Self {
        Self {
            prompt: Arc::new(prompt.clone()),
            nonce: GuardNonce::fresh(),
            store: store.clone(),
            execution: Arc::from(config.execution.as_str()),
            args: Arc::from(args),
            limits: config.limits,
            observer: Arc::clone(&config.observer),
            debug: config.debug.clone(),
            turns: Arc::new(AtomicU32::new(0)),
            ids: Arc::new(AtomicU64::new(0)),
            shared: Arc::new(shared),
            shared_tools,
            bindings: Arc::new(ToolBindings::default()),
            models: Arc::new(ModelBindings::from_parts(Vec::new(), None)),
            analysis: Arc::new(ToolAnalysis::default()),
            when: Arc::from(""),
        }
    }

    /// The prompt this run executes.
    pub(crate) fn prompt(&self) -> &Prompt {
        &self.prompt
    }

    /// The run's untrusted-envelope nonce.
    pub(crate) fn nonce(&self) -> &GuardNonce {
        &self.nonce
    }

    /// The run-scoped store.
    pub(crate) fn store(&self) -> &StoreRef {
        &self.store
    }

    /// The execution identifier every observation carries.
    pub(crate) fn execution(&self) -> &str {
        &self.execution
    }

    /// The run's argument string.
    pub(crate) fn args(&self) -> &str {
        &self.args
    }

    /// The run's resource limits.
    pub(crate) fn limits(&self) -> RunLimits {
        self.limits
    }

    /// The run's observer handle.
    pub(crate) fn observer(&self) -> &Arc<dyn Observer> {
        &self.observer
    }

    /// The opt-in raw request/response capture sink.
    pub(crate) fn debug(&self) -> Option<&Arc<dyn DebugCapture>> {
        self.debug.as_ref()
    }

    /// The model-turn counter this context advances.
    pub(crate) fn turns(&self) -> &Arc<AtomicU32> {
        &self.turns
    }

    /// The run-global execution-id counter.
    pub(crate) fn ids(&self) -> &Arc<AtomicU64> {
        &self.ids
    }

    /// The run's shared tool registry handle.
    pub(crate) fn shared_tools(&self) -> &SharedTools {
        &self.shared_tools
    }

    /// The frozen prompt-level tool bindings.
    pub(crate) fn bindings(&self) -> &ToolBindings {
        &self.bindings
    }

    /// The frozen prompt-level model bindings.
    pub(crate) fn models(&self) -> &ModelBindings {
        &self.models
    }

    /// The prompt's tool-scope analysis.
    pub(crate) fn analysis(&self) -> &ToolAnalysis {
        &self.analysis
    }

    /// The resolved per-section tool-loop cap: the frontmatter's
    /// `max_tool_iterations` over the limits default.
    pub(crate) fn max_tool_iterations(&self) -> usize {
        self.prompt
            .frontmatter
            .max_tool_iterations
            .resolve(self.limits.tool_iterations().get() as usize)
    }

    /// The run's top-level section count, reported as `sys.section_count`.
    pub(crate) fn section_count(&self) -> usize {
        self.prompt.sections.len()
    }

    /// The H1-to-walk handoff: the live values H1 produced (the frozen
    /// bindings and models, the scope analysis) plus the walk's start
    /// timestamp, set on a cheap clone so the context H1 saw stays
    /// untouched.
    #[must_use]
    pub(crate) fn with_walk_state(
        &self,
        bindings: &ToolBindings,
        models: &ModelBindings,
        analysis: &ToolAnalysis,
        when: &str,
    ) -> Self {
        let mut ctx = self.clone();
        ctx.bindings = Arc::new(bindings.clone());
        ctx.models = Arc::new(models.clone());
        ctx.analysis = Arc::new(analysis.clone());
        ctx.when = Arc::from(when);
        ctx
    }

    /// The context a contained chain runs under: `args` in place of the
    /// run's own, because an `execute` call's explicit input overrides the
    /// run's args for the chain.
    #[must_use]
    pub(crate) fn with_args(&self, args: &str) -> Self {
        let mut ctx = self.clone();
        ctx.args = Arc::from(args);
        ctx
    }

    /// The context a fanout's arms run under: the proxy observer/debug over
    /// the bounded side channels and the fanout's fresh turn counter in
    /// place of the run's own, so arm reporting stays report-only and arm
    /// turns count against the fanout's cap.
    #[must_use]
    pub(crate) fn with_effective_handles(
        &self,
        observer: Arc<dyn Observer>,
        debug: Option<Arc<dyn DebugCapture>>,
        turns: Arc<AtomicU32>,
    ) -> Self {
        let mut ctx = self.clone();
        ctx.observer = observer;
        ctx.debug = debug;
        ctx.turns = turns;
        ctx
    }

    /// The borrowed VM-setup inputs both engine drivers share, sourcing the
    /// run-wide slots (`args`, `store`, `observer`, `shared`) from this
    /// context; the driver supplies only its own deltas: the `sys` JSON, the
    /// incoming reply, the seed, the store-write scope (a fanout arm's
    /// identity; `None` on the walk), and the section name.
    pub(crate) fn vm_setup<'a>(
        &'a self,
        sys: &'a serde_json::Value,
        last_reply: Option<&'a str>,
        seed: VmSeed<'a>,
        write_scope: Option<WriteScope>,
        section_name: &'a str,
    ) -> SectionVmSetup<'a> {
        SectionVmSetup {
            args: &self.args,
            sys,
            store: &self.store,
            last_reply,
            seed,
            write_scope,
            observer_arc: &self.observer,
            section_name,
            shared: &self.shared,
        }
    }

    /// The `sys` JSON for one section or arm of this run: a fresh `now`
    /// timestamp under the walk's `when`, with the driver supplying only the
    /// next value from the run-global id counter and the section name.
    ///
    /// # Errors
    /// Returns [`Error::TimestampFormat`](crate::Error::TimestampFormat) when
    /// the current time fails to format.
    pub(crate) fn sys_json(&self, id: u64, section_name: &str) -> Result<serde_json::Value> {
        let now = now_rfc3339_checked()?;
        Ok(sys_json(
            &self.when,
            &now,
            id,
            section_name,
            &self.execution,
            self.section_count(),
        ))
    }

    /// Installs the infer hook both engine drivers share, sourcing every
    /// run-wide slot from this context; the driver supplies only its client
    /// snapshot and the section name. The handed client (or the environment)
    /// is wrapped in the lazy [`GatewaySource`], with no live H1 bindings.
    pub(crate) fn attach_infer_hook(
        &self,
        vm: &SectionVm,
        client: Option<GatewayClient>,
        section_name: &str,
    ) {
        super::tools::attach_infer_hook(
            vm,
            GatewaySource::from_optional(client, self.limits),
            Arc::clone(&self.observer),
            self.debug.clone(),
            &self.execution,
            section_name,
            &self.turns,
            None,
        );
    }
}

impl fmt::Debug for RunContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunContext")
            .field("prompt", &self.prompt)
            .field("nonce", &self.nonce)
            .field("store", &"<StoreRef>")
            .field("execution", &self.execution)
            .field("args", &self.args)
            .field("limits", &self.limits)
            .field("observer", &"<dyn Observer>")
            .field("debug", &self.debug.as_ref().map(|_| "<dyn DebugCapture>"))
            .field("turns", &self.turns)
            .field("ids", &self.ids)
            .field("shared", &self.shared)
            .field("shared_tools", &self.shared_tools)
            .field("bindings", &self.bindings)
            .field("models", &self.models)
            .field("analysis", &self.analysis)
            .field("when", &self.when)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::NullObserver;

    fn test_prompt() -> Prompt {
        let source = concat!(
            "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n",
            "# Title\n\n## Only\n\ndone\n",
        );
        Prompt::parse(source, "run-context-test", &NullObserver).expect("the test prompt parses")
    }

    fn test_context(prompt: &Prompt) -> RunContext {
        RunContext::new(
            prompt,
            "",
            &StoreRef::memory(),
            SharedTools::default(),
            LuaProgram::empty().expect("the empty chunk compiles"),
            &RunConfig::new("run-context-test"),
        )
    }

    #[test]
    fn new_builds_a_context_over_the_prompt() {
        let prompt = test_prompt();
        let ctx = test_context(&prompt);
        assert_eq!(ctx.prompt().title, prompt.title);
    }

    #[test]
    fn accessor_returns_the_run_prompt() {
        let prompt = test_prompt();
        let ctx = test_context(&prompt);
        assert_eq!(ctx.prompt(), &prompt);
    }

    #[test]
    fn clones_share_the_prompt_allocation() {
        let ctx = test_context(&test_prompt());
        let clone = ctx.clone();
        assert!(Arc::ptr_eq(&ctx.prompt, &clone.prompt));
    }

    #[test]
    fn derived_values_come_from_the_prompt_and_limits() {
        let prompt = test_prompt();
        let ctx = test_context(&prompt);
        assert_eq!(ctx.section_count(), prompt.sections.len());
        assert_eq!(ctx.max_tool_iterations(), 24);
    }

    #[test]
    fn forks_swap_only_their_own_fields() {
        let ctx = test_context(&test_prompt());
        let chain = ctx.with_args("chain-args");
        assert_eq!(chain.args(), "chain-args");
        assert!(Arc::ptr_eq(&ctx.prompt, &chain.prompt));
        assert_eq!(ctx.args(), "");

        let turns = Arc::new(AtomicU32::new(7));
        let arm = ctx.with_effective_handles(Arc::new(NullObserver), None, Arc::clone(&turns));
        assert!(Arc::ptr_eq(arm.turns(), &turns));
        assert!(Arc::ptr_eq(&ctx.prompt, &arm.prompt));
    }
}
