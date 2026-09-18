//! The execute subtree's ambient run state.
//!
//! [`RunState`] is built once in [`run`](super::run) and travels through
//! the execute subtree as parameter one (`ctx: &RunState`). The
//! invariant: a new run-scoped concern becomes a field here, never a new
//! parameter. Per-call data (a section, a `var` snapshot) stays
//! in parameters or on the per-section frame.

use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::{Arc, Mutex};

use promptforge_parser::ModelKeyword;

use crate::Result;
use crate::debug::DebugCapture;
use crate::input::InputBroker;
use crate::lua::{LuaProgram, ToolBinding, ToolSet, ToolView};
use crate::model::{ModelBinding, ModelInvocation, ModelSet, ModelView};
use crate::observe::Observer;
use crate::parser::Prompt;
use crate::store::{Access, VfsRef};
use crate::untrusted::GuardNonce;

use super::config::{RunContext, RunLimits};
use super::section_vm::{SectionVmSetup, VmSeed};
use super::support::{now_rfc3339_checked, sys_json};

/// Builds the run's shared tool set from the prepared bindings: every
/// filled slot becomes a binding carrying its resolved implementation, so
/// run-time execution never consults the assembled catalog again. Unfilled
/// slots produce no binding: advertising or calling the alias fails at run
/// time, exactly as prepare's report promised.
fn bound_tool_set(prompt: &Prompt, ctx: &RunContext) -> ToolSet {
    let mut set = ToolSet::default();
    for (alias, _) in prompt.frontmatter().tools().iter() {
        let Some(tool) = ctx.tool_bindings.resolve(alias) else {
            continue;
        };
        // The exact path says nothing prose-like; the tool's own catalog
        // text stands in as the binding's description.
        set.bindings.push(ToolBinding {
            alias: alias.to_owned(),
            description: tool.description().to_owned(),
            id: tool.id(),
            model_description: None,
            tool: Arc::clone(tool),
            output_kind: crate::lua::ToolOutputKind::Plain,
        });
    }
    set
}

/// The keyword's stable kebab-case spelling, journaled onto the binding as
/// the role's capability set.
fn keyword_name(keyword: ModelKeyword) -> &'static str {
    match keyword {
        ModelKeyword::Thinking => "thinking",
        ModelKeyword::NoThinking => "no-thinking",
        ModelKeyword::Frontier => "frontier",
        ModelKeyword::Fast => "fast",
        ModelKeyword::Small => "small",
        ModelKeyword::Creative => "creative",
        ModelKeyword::Chat => "chat",
        // The vocabulary is closed today; a future keyword reports its
        // debug spelling rather than breaking the fill.
        _ => "unknown",
    }
}

/// Builds the run's shared model set from the prepared bindings: every
/// filled role becomes a binding under its label, carrying the role's
/// keyword set (the handle's `capabilities`) and the hard-keyword thinking
/// switch as the frozen invocation. Unfilled roles produce no binding:
/// `models.use` on the label fails at run time.
fn bound_model_set(prompt: &Prompt, ctx: &RunContext) -> ModelSet {
    let mut set = ModelSet::default();
    for (label, role) in prompt.frontmatter().models().iter() {
        let Some(descriptor) = ctx.model_bindings.resolve(label) else {
            continue;
        };
        let mut thinking = None;
        for keyword in role.keywords() {
            match keyword {
                ModelKeyword::Thinking => thinking = Some(true),
                ModelKeyword::NoThinking => thinking = Some(false),
                _ => {}
            }
        }
        let binding = ModelBinding::new(
            label,
            role.description()
                .unwrap_or_else(|| descriptor.description()),
            descriptor.id().clone(),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking,
            },
            descriptor.context(),
        )
        .with_capabilities(
            role.keywords()
                .iter()
                .map(|keyword| keyword_name(*keyword))
                .map(str::to_owned)
                .collect(),
        );
        set.bindings.push(binding);
    }
    set
}

/// Derives a section's `argv` from the args string under the prompt's
/// declaration: a default-declared prompt wraps the interface prose into
/// the default shape (`argv.prose`, with the empty string present, not
/// absent); a structured declaration parses the string as JSON, and a parse
/// failure or a JSON `null` reads as nil (`if argv then` is the malformed
/// check). The executor never hard-errors on shape.
fn derive_argv(prompt: &Prompt, args: &str) -> Option<serde_json::Value> {
    if prompt.frontmatter().args().is_default() {
        return Some(serde_json::json!({ "prose": args }));
    }
    match serde_json::from_str(args) {
        Ok(serde_json::Value::Null) | Err(_) => None,
        Ok(value) => Some(value),
    }
}

/// The ambient state one run shares across the execute subtree.
///
/// Immutable for the run's lifetime and cheap to clone: every field is
/// shared ownership or `Copy`, so a clone points at the same run state.
/// The three sanctioned forks: [`with_walk_state`](Self::with_walk_state)
/// at the H1-to-walk handoff,
/// [`with_effective_handles`](Self::with_effective_handles) for a fanout's
/// proxy reporting handles, and [`with_args`](Self::with_args) carrying a
/// `call` call's args override into its contained chain.
#[derive(Clone)]
pub(crate) struct RunState {
    /// The prompt this run executes.
    prompt: Arc<Prompt>,
    /// The untrusted-envelope nonce, minted once here so every wrap in the
    /// run shares it.
    nonce: GuardNonce,
    /// The run's VFS handle: carries the store mount backing every
    /// section's Lua `store` table. Chain steps acquire or spawn their
    /// access capabilities from it.
    vfs: VfsRef,
    /// The execution identifier every observation carries.
    execution: Arc<str>,
    /// The run's argument string for `{{ args }}` substitution.
    args: Arc<str>,
    /// The run's `argv`: the parsed form of `args` (`None` installs nil).
    /// At construction this is the derived value the H1 pass starts from;
    /// the walk's fork carries the value H1 left behind at the freeze, so
    /// an H1 repair reaches every downstream section.
    argv: Option<Arc<serde_json::Value>>,
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
    /// The run's tool set as a read-only view: built from the prepared
    /// bindings at construction. The trait exposes no write methods; the
    /// only writer is `tools.always` (a prompt-wide fact) through the
    /// concrete handle the section VMs share.
    tools: Arc<dyn ToolView>,
    /// The concrete handle behind `tools`, shared with every section VM
    /// (H1 included). Readers outside the VM layer go through the view.
    tool_set: Arc<Mutex<ToolSet>>,
    /// The run's model set as a read-only view: built from the prepared
    /// bindings at construction. The trait exposes no write methods; the
    /// only writer is `models.default` (a prompt-wide fact) through the
    /// concrete handle the section VMs share.
    models: Arc<dyn ModelView>,
    /// The concrete handle behind `models`, shared with every section VM
    /// (H1 included). Readers outside the VM layer go through the view.
    model_set: Arc<Mutex<ModelSet>>,
    /// The walk's start timestamp, stamped into every section's `sys.when`;
    /// empty until the walk starts (H1 stamps its own `now`).
    when: Arc<str>,
    /// The run's input broker, when the host configured one; `None` is the
    /// unavailable-fallback policy.
    input: Option<Arc<dyn InputBroker>>,
    /// The run's host-state snapshot provider; its presence is the
    /// Agent-window context (the `ui()` global plus raw-id `models.get`).
    ui: Option<Arc<dyn Fn() -> serde_json::Value + Send + Sync>>,
    /// The host's live streaming-delta callback, forwarded by every model
    /// round; `None` drops deltas at the leaf.
    on_delta: Option<Arc<dyn Fn(crate::client::StreamDelta) + Send + Sync>>,
}

impl RunState {
    /// Builds the context for one run of `prompt`. The turn and id counters
    /// are minted here (both start at zero), as are the run's shared tool
    /// and model sets - built from the prepared bindings on `ctx` (empty on
    /// a caller-built context that never passed through
    /// [`Environment::prepare`](super::Environment::prepare), which runs
    /// capability-free); `when` starts empty and takes its live value at the
    /// H1-to-walk handoff.
    #[must_use]
    pub(crate) fn new(
        prompt: &Prompt,
        args: &str,
        vfs: &VfsRef,
        shared: LuaProgram,
        ctx: &RunContext,
    ) -> Self {
        let tool_set = Arc::new(Mutex::new(bound_tool_set(prompt, ctx)));
        let model_set = Arc::new(Mutex::new(bound_model_set(prompt, ctx)));
        Self {
            prompt: Arc::new(prompt.clone()),
            nonce: GuardNonce::fresh(),
            vfs: vfs.clone(),
            execution: Arc::from(ctx.name.as_str()),
            args: Arc::from(args),
            argv: derive_argv(prompt, args).map(Arc::from),
            limits: ctx.limits,
            observer: Arc::clone(&ctx.observer),
            debug: ctx.debug.clone(),
            turns: Arc::new(AtomicU32::new(0)),
            ids: Arc::new(AtomicU64::new(0)),
            shared: Arc::new(shared),
            tools: tool_set.clone(),
            tool_set,
            models: model_set.clone(),
            model_set,
            when: Arc::from(""),
            input: ctx.input.clone(),
            ui: ctx.ui.clone(),
            on_delta: ctx.on_delta.clone(),
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

    /// The run's VFS handle.
    pub(crate) fn vfs(&self) -> &VfsRef {
        &self.vfs
    }

    /// The execution identifier every observation carries.
    pub(crate) fn execution(&self) -> &str {
        &self.execution
    }

    /// The run's argument string.
    pub(crate) fn args(&self) -> &str {
        &self.args
    }

    /// The run's `argv`: the parsed form of the args string, or `None`
    /// (nil) when it did not parse. On the walk this is the value H1 left
    /// behind at the freeze.
    pub(crate) fn argv(&self) -> Option<&serde_json::Value> {
        self.argv.as_deref()
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

    /// The run's tool set, read-only.
    ///
    /// Unused until the `models.loop` step reads the call-time tool scope.
    #[expect(
        dead_code,
        reason = "unused until the models.loop step reads the call-time tool scope"
    )]
    pub(crate) fn tools(&self) -> &dyn ToolView {
        &*self.tools
    }

    /// The concrete handle behind the tools view, shared with every
    /// section VM the run constructs.
    pub(crate) fn tool_set(&self) -> Arc<Mutex<ToolSet>> {
        Arc::clone(&self.tool_set)
    }

    /// An owned snapshot of the run's tool set (bindings plus `always`),
    /// read through the view.
    ///
    /// # Errors
    /// Returns [`Error::Lua`](crate::Error::Lua) if the set's mutex is
    /// poisoned.
    pub(crate) fn tool_set_snapshot(&self) -> Result<ToolSet> {
        Ok(ToolSet::from_parts(
            self.tools.bindings()?,
            self.tools.always()?,
        ))
    }

    /// The run's model set, read-only.
    pub(crate) fn models(&self) -> &dyn ModelView {
        &*self.models
    }

    /// The concrete handle behind the models view, shared with every
    /// section VM the run constructs.
    pub(crate) fn model_set(&self) -> Arc<Mutex<ModelSet>> {
        Arc::clone(&self.model_set)
    }

    /// The resolved per-section tool-loop cap: the frontmatter's
    /// `max_tool_iterations` over the limits default.
    pub(crate) fn max_tool_iterations(&self) -> usize {
        self.prompt
            .frontmatter()
            .max_tool_iterations()
            .resolve(self.limits.tool_iterations().get() as usize)
    }

    /// The run's top-level section count, reported as `sys.section_count`.
    pub(crate) fn section_count(&self) -> usize {
        self.prompt.sections().len()
    }

    /// The run's input broker, when the host configured one.
    pub(crate) fn input_broker(&self) -> Option<&Arc<dyn InputBroker>> {
        self.input.as_ref()
    }

    /// The host's live streaming-delta callback, when one was configured.
    pub(crate) fn on_delta(
        &self,
    ) -> Option<&Arc<dyn Fn(crate::client::StreamDelta) + Send + Sync>> {
        self.on_delta.as_ref()
    }

    /// The H1-to-walk handoff: the walk's start timestamp and the `argv`
    /// H1 left behind at the freeze, set on a cheap clone so the context
    /// H1 saw stays untouched. The tool and model sets
    /// need no delta: they were built from the prepared bindings at
    /// construction, and H1's prompt-wide records (`tools.always`,
    /// `models.default`) landed in the same shared sets the views read.
    #[must_use]
    pub(crate) fn with_walk_state(&self, when: &str, argv: Option<serde_json::Value>) -> Self {
        let mut ctx = self.clone();
        ctx.when = Arc::from(when);
        ctx.argv = argv.map(Arc::from);
        ctx
    }

    /// The context a contained chain runs under: `args` in place of the
    /// run's own, because a `call` call's explicit input overrides the
    /// run's args for the chain - and `argv` re-derives from the chain's
    /// args, so the chain sees the parsed form of what it was passed.
    #[must_use]
    pub(crate) fn with_args(&self, args: &str) -> Self {
        let mut ctx = self.clone();
        ctx.argv = derive_argv(&self.prompt, args).map(Arc::from);
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
    /// run-wide slots (`args`, `observer`, `shared`) from this
    /// context; the driver supplies only its own deltas: the `sys` JSON,
    /// the seed, the chain step's access capability (the walk's own, a
    /// call chain's borrowed parent capability, a fanout arm's spawned
    /// one), and the section name.
    pub(crate) fn vm_setup<'a>(
        &'a self,
        sys: &'a serde_json::Value,
        seed: VmSeed<'a>,
        access: &'a Arc<Access>,
        section_name: &'a str,
    ) -> SectionVmSetup<'a> {
        SectionVmSetup {
            args: &self.args,
            argv: self.argv(),
            argv_writable: false,
            sys,
            access,
            seed,
            observer_arc: &self.observer,
            section_name,
            shared: &self.shared,
            ui: self.ui.as_ref(),
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
}

impl fmt::Debug for RunState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunState")
            .field("prompt", &self.prompt)
            .field("nonce", &self.nonce)
            .field("vfs", &"<VfsRef>")
            .field("execution", &self.execution)
            .field("args", &self.args)
            .field("argv", &self.argv)
            .field("limits", &self.limits)
            .field("observer", &"<dyn Observer>")
            .field("debug", &self.debug.as_ref().map(|_| "<dyn DebugCapture>"))
            .field("turns", &self.turns)
            .field("ids", &self.ids)
            .field("shared", &self.shared)
            .field("tools", &"<dyn ToolView>")
            .field("tool_set", &self.tool_set)
            .field("models", &"<dyn ModelView>")
            .field("model_set", &self.model_set)
            .field("when", &self.when)
            .field("input", &self.input.is_some())
            .field("ui", &self.ui.is_some())
            .field("on_delta", &self.on_delta.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::NullObserver;

    fn test_prompt() -> Prompt {
        let source = concat!(
            "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n",
            "# Title\n\n## Only\n\ndone\n",
        );
        Prompt::parse(source, "run-context-test", &NullObserver::default())
            .expect("the test prompt parses")
    }

    fn test_context(prompt: &Prompt) -> RunState {
        RunState::new(
            prompt,
            "",
            &promptforge_vfs::empty(),
            LuaProgram::empty().expect("the empty chunk compiles"),
            &RunContext::new("run-context-test"),
        )
    }

    #[test]
    fn new_builds_a_context_over_the_prompt() {
        let prompt = test_prompt();
        let ctx = test_context(&prompt);
        assert_eq!(ctx.prompt().title(), prompt.title());
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
        assert_eq!(ctx.section_count(), prompt.sections().len());
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
        let arm =
            ctx.with_effective_handles(Arc::new(NullObserver::default()), None, Arc::clone(&turns));
        assert!(Arc::ptr_eq(arm.turns(), &turns));
        assert!(Arc::ptr_eq(&ctx.prompt, &arm.prompt));
    }
}
