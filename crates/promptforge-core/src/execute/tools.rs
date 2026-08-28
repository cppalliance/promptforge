//! The infer hook that installs a section VM's nested-inference bridge.
//!
//! One `infer` shape only: a single direct, tool-free gateway round on a
//! fresh conversation. `handle:infer(prompt)` runs it with the handle's
//! frozen binding; `models.infer(prompt)` resolves the section's current
//! model and runs the same path. Neither form advertises tools, sets
//! `reply`, or touches `sys`. A Lua block that needs tools uses `execute`
//! on a section.

use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};

use crate::Error;
use crate::cancel;
use crate::client::{Completion, CompletionResult, GatewayClient, Message};
use crate::debug::{DebugCapture, DebugEvent};
use crate::lua::{ModelInferHook, ModelRuntime, ModelsInferHook, SectionVm, resolve_model_binding};
use crate::model::{ModelBinding, ModelView};
use crate::observe::{Observer, detail};

use super::gateway::GatewaySource;
use super::support::{advance_turn, bridge_blocking};

/// Reports one completed infer round exactly like a single prose round and
/// renders its text: the turn advance, the debug capture pair, the
/// completion and truncation observations, and the no-tools-advertised
/// violation check. Both infer drivers - the legacy hook's bridged round
/// and the scheduler's spawned round - share this tail.
fn accept_infer_completion(
    completion: Completion,
    observer: &dyn Observer,
    debug: Option<&dyn DebugCapture>,
    execution: &str,
    section: &str,
    turns: &AtomicU32,
) -> Result<String, Error> {
    let turn = advance_turn(turns);
    if let Some(capture) = debug {
        capture.on_event(
            execution,
            section,
            turn,
            DebugEvent::Request {
                body: completion.request_body,
            },
        );
        capture.on_event(
            execution,
            section,
            turn,
            DebugEvent::Response {
                body: completion.response_body.clone(),
                finish_reason: completion.finish_reason.clone(),
                reasoning_content: completion.reasoning_content.clone(),
            },
        );
    }
    observer.observe(execution, section, detail::MODEL_TURN_COMPLETED);

    match completion.result {
        CompletionResult::Text(text) => {
            if completion.finish_reason.as_deref() == Some("length") {
                observer.observe(execution, section, detail::MODEL_TURN_TRUNCATED);
            }
            Ok(text)
        }
        // No tools were advertised, so a tool-call turn is a backend
        // protocol violation rather than something to dispatch.
        CompletionResult::ToolCalls(_) => Err(Error::Lua(
            "model inference received tool calls but no tools were advertised".to_owned(),
        )),
    }
}

/// The one infer shape as an async round: a single direct, tool-free
/// gateway call on a fresh conversation with `binding`, reported exactly
/// like one prose round.
///
/// The scheduler's leaf dispatch drives this on a spawned task, so
/// cancellation is the driver aborting the task mid-round - no
/// `MODEL_TURN_FAILED` fires for an aborted round, matching the legacy
/// hook's interrupted path.
// Consumed by the scheduler driver until the flip; exercised today by the
// scheduler tests.
#[allow(dead_code)]
#[expect(
    clippy::too_many_arguments,
    reason = "the one infer round keeps the client, binding, prompt, and the frame's reporting handles explicit and linear, mirroring the legacy hook's InferContext fields"
)]
pub(crate) async fn infer_round(
    client: &GatewayClient,
    binding: &ModelBinding,
    prompt: &str,
    observer: &dyn Observer,
    debug: Option<&dyn DebugCapture>,
    execution: &str,
    section: &str,
    turns: &AtomicU32,
) -> Result<String, Error> {
    let completion_options = binding.completion_options();
    let conversation = [Message::user(prompt)];
    let completion = match client
        .complete(&conversation, None, &completion_options)
        .await
    {
        Ok(completion) => completion,
        Err(error) => {
            observer.observe(execution, section, detail::MODEL_TURN_FAILED);
            return Err(Error::from(error));
        }
    };
    accept_infer_completion(completion, observer, debug, execution, section, turns)
}

/// Shared context for nested inference from Lua.
///
/// Carries the gateway client, observer, and debug sink so each infer call
/// reports exactly like one prose round.
pub(crate) struct InferContext {
    client: GatewaySource,
    observer: Arc<dyn Observer>,
    /// Owned debug sink so nested inference capture is not lost (F4).
    debug: Option<Arc<dyn DebugCapture>>,
    execution: String,
    section: String,
    turns: Arc<AtomicU32>,
    /// The run's model set, read through the view: bindings-so-far during
    /// the live H1 pass, the frozen set in H2 - one read path for both.
    models: Arc<dyn ModelView>,
    /// The section's `models.use` selection runtime for `models.infer`.
    model_runtime: Arc<Mutex<ModelRuntime>>,
}

impl InferContext {
    /// Resolves the section's current model binding for `models.infer`.
    ///
    /// The current alias is the H2 `models.use` selection, else the
    /// prompt-wide `models.default` baseline.
    fn current_model_binding(&self) -> mlua::Result<ModelBinding> {
        resolve_model_binding(self.models.as_ref(), &self.model_runtime)
            .map_err(mlua::Error::external)?
            .ok_or_else(|| {
                mlua::Error::external(Error::ModelRequired {
                    section: self.section.clone(),
                })
            })
    }

    /// Runs the one infer shape: one direct, tool-free gateway round on a
    /// fresh conversation with `binding`.
    ///
    /// No schemas are advertised, and neither `reply` nor
    /// `sys.reply_finish_reason` is touched. Turn counting, observation, and
    /// debug capture match a single prose round so nested inference is not
    /// lost (observe F1, F4): the reporting tail is the shared
    /// [`accept_infer_completion`].
    fn infer_direct(&self, binding: &ModelBinding, prompt: &str) -> mlua::Result<String> {
        let completion_options = binding.completion_options();
        let client = self.client.resolve().map_err(mlua::Error::external)?;
        let conversation = [Message::user(prompt)];
        let completion = bridge_blocking(async {
            tokio::select! {
                biased;
                () = cancel::wait_cancelled() => Err(Error::Interrupted),
                result = client.complete(&conversation, None, &completion_options) => {
                    result.map_err(Error::from)
                }
            }
        });
        let completion = match completion {
            Ok(completion) => completion,
            Err(Error::Interrupted) => return Err(mlua::Error::external(Error::Interrupted)),
            Err(error) => {
                self.observer
                    .observe(&self.execution, &self.section, detail::MODEL_TURN_FAILED);
                return Err(mlua::Error::external(error));
            }
        };
        accept_infer_completion(
            completion,
            self.observer.as_ref(),
            self.debug.as_deref(),
            &self.execution,
            &self.section,
            &self.turns,
        )
        .map_err(mlua::Error::external)
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "infer hook installation threads the same borrowed run context fanout already carries"
)]
pub(crate) fn attach_infer_hook(
    vm: &SectionVm,
    client: GatewaySource,
    observer: Arc<dyn Observer>,
    debug: Option<Arc<dyn DebugCapture>>,
    execution: &str,
    section: &str,
    turns: &Arc<AtomicU32>,
    models: Arc<dyn ModelView>,
) {
    let ctx = Arc::new(InferContext {
        client,
        // The run's owned observer reaches the nested infer hook, so
        // observations from nested inference are not lost (observe F1).
        observer,
        // The run's owned debug sink likewise reaches nested inference (F4).
        debug,
        execution: execution.to_owned(),
        section: section.to_owned(),
        turns: Arc::clone(turns),
        models,
        model_runtime: Arc::clone(&vm.model_runtime),
    });
    let direct = Arc::clone(&ctx);
    // `handle:infer` runs the direct path with the handle's frozen binding.
    let hook: ModelInferHook =
        Arc::new(move |_, binding, prompt| ctx.infer_direct(binding, prompt));
    vm.set_infer_hook(hook);
    // `models.infer` resolves the section's current model, then runs the
    // same direct path.
    let models_hook: ModelsInferHook = Arc::new(move |_, prompt| {
        let binding = direct.current_model_binding()?;
        direct.infer_direct(&binding, prompt)
    });
    vm.set_models_infer_hook(models_hook);
}
