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
use crate::client::{CompletionResult, Message};
use crate::debug::{DebugCapture, DebugEvent};
use crate::lua::{
    LiveBindingProducer, ModelBindings, ModelInferHook, ModelRuntime, ModelsInferHook, SectionVm,
    resolve_model_binding,
};
use crate::model::ModelBinding;
use crate::observe::{Observer, detail};

use super::gateway::GatewaySource;
use super::support::{advance_turn, bridge_blocking};

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
    live_bindings: Option<LiveBindingProducer>,
    /// Frozen prompt-level model bindings for `models.infer` resolution.
    model_bindings: ModelBindings,
    /// The section's `models.use` selection runtime for `models.infer`.
    model_runtime: Arc<Mutex<ModelRuntime>>,
}

impl InferContext {
    /// Resolves the section's current model binding for `models.infer`.
    ///
    /// On the live H1 path the bindings are still being recorded, so the
    /// snapshot comes from the run's producer; on the H2 path the VM's frozen
    /// bindings are used. The current alias is the H2 `models.use` selection,
    /// else the prompt-wide `models.default` baseline.
    fn current_model_binding(&self) -> mlua::Result<ModelBinding> {
        let bindings = if let Some(live) = &self.live_bindings {
            live.bindings().map_err(mlua::Error::external)?.1
        } else {
            self.model_bindings.clone()
        };
        resolve_model_binding(&bindings, &self.model_runtime)
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
    /// lost (observe F1, F4).
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

        let turn = advance_turn(&self.turns);
        if let Some(capture) = self.debug.as_deref() {
            capture.on_event(
                &self.execution,
                &self.section,
                turn,
                DebugEvent::Request {
                    body: completion.request_body,
                },
            );
            capture.on_event(
                &self.execution,
                &self.section,
                turn,
                DebugEvent::Response {
                    body: completion.response_body.clone(),
                    finish_reason: completion.finish_reason.clone(),
                    reasoning_content: completion.reasoning_content.clone(),
                },
            );
        }
        self.observer
            .observe(&self.execution, &self.section, detail::MODEL_TURN_COMPLETED);

        match completion.result {
            CompletionResult::Text(text) => {
                if completion.finish_reason.as_deref() == Some("length") {
                    self.observer.observe(
                        &self.execution,
                        &self.section,
                        detail::MODEL_TURN_TRUNCATED,
                    );
                }
                Ok(text)
            }
            // No tools were advertised, so a tool-call turn is a backend
            // protocol violation rather than something to dispatch.
            CompletionResult::ToolCalls(_) => Err(mlua::Error::external(
                "model inference received tool calls but no tools were advertised",
            )),
        }
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
    live_bindings: Option<LiveBindingProducer>,
) {
    let (model_bindings, model_runtime) = vm.model_bag_handles();
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
        live_bindings,
        model_bindings,
        model_runtime,
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
