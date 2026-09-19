//! The internal performer table: today's spawned leaf work, behind the
//! effect boundary.
//!
//! A leaf arm no longer spawns anything. It builds an [`Effect`] and the
//! scheduler hands it here; the table spawns the one task that performs
//! it - a gateway round, a tool call, a broker wait, a blocking-pool store
//! operation, a sleep - and that task posts the raw [`EffectAnswer`] on
//! the answer channel under the effect's id. Nothing here touches
//! scheduler state or a Lua value, so the driver future stays `Send`; and
//! nothing here emits an event: the answer's meaning (the round's events,
//! the trust rule, the store observation) is decided when the driver
//! applies it, on its own thread.
//!
//! The table is the engine's stand-in for a host. It resolves what a host
//! would resolve - the gateway client from the run's configured source,
//! a tool implementation from the run's catalog by the effect's tool id,
//! the input broker from the run context - and performs with them.

use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::client::GatewayClient;
use crate::execute::context::RunState;
use crate::execute::gateway::GatewaySource;
#[cfg(test)]
use crate::execute::run::EffectRecord;
use crate::execute::run::{Effect, EffectAnswer, EffectId};
use crate::lua::run_store_op;
use crate::store::Store;
use crate::{Error, Result};

/// The send half every performer posts its answer to.
type AnswerSender = mpsc::UnboundedSender<(EffectId, EffectAnswer)>;

/// The performer table for one run.
pub(super) struct Performers<'a> {
    /// The run context the performers resolve their resources from.
    ctx: &'a RunState,
    /// The answer channel's send half. The channel is unbounded: each
    /// performer sends exactly once, and the in-flight count is already
    /// bounded by the chains that produced the effects.
    tx: AnswerSender,
    /// The run's gateway source, resolved on the first `Chat` effect so a
    /// construction error surfaces at first use rather than being
    /// swallowed.
    gateway: GatewaySource,
    /// The resolved client, cached for the run.
    client: Option<GatewayClient>,
    /// Test-only: the record of every effect performed, in issue order.
    #[cfg(test)]
    tap: Option<Arc<Mutex<Vec<EffectRecord>>>>,
}

impl<'a> Performers<'a> {
    /// Builds the table for one run over `ctx` and returns it with the
    /// answer channel's receive half, which the driver awaits. `client`
    /// is the run's gateway client, if the caller supplied one; otherwise
    /// the first `Chat` effect builds one from the environment.
    pub(super) fn new(
        ctx: &'a RunState,
        client: Option<GatewayClient>,
    ) -> (Self, mpsc::UnboundedReceiver<(EffectId, EffectAnswer)>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let performers = Self {
            ctx,
            tx,
            gateway: GatewaySource::from_optional(client, ctx.limits()),
            client: None,
            #[cfg(test)]
            tap: None,
        };
        (performers, rx)
    }

    /// The run's gateway client, resolved from the source on first use and
    /// cached.
    ///
    /// # Errors
    /// Returns the client's construction error when the environment
    /// source cannot build one.
    fn client(&mut self) -> Result<GatewayClient> {
        if let Some(client) = &self.client {
            return Ok(client.clone());
        }
        let client = self.gateway.resolve()?;
        self.client = Some(client.clone());
        Ok(client)
    }

    /// Performs one effect: spawns the leaf work that will post the
    /// effect's answer under `id`, and returns its join handle so the
    /// scheduler can abort or drain it.
    ///
    /// # Errors
    /// Returns the client's construction error for a `Chat` effect when no
    /// client can be built, or [`Error::Internal`] for a `ToolCall` whose
    /// tool id is not in the run's catalog, a `UserInput` with no broker
    /// configured, or a `Timer` whose seconds `Duration` cannot hold -
    /// each of which the issuing arm has already ruled out.
    pub(super) fn perform(&mut self, id: EffectId, effect: Effect) -> Result<JoinHandle<()>> {
        #[cfg(test)]
        if let Some(tap) = &self.tap {
            tap.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(effect.record());
        }
        let tx = self.tx.clone();
        match effect {
            Effect::Chat {
                messages,
                tools,
                options,
                stream,
                ..
            } => {
                let client = self.client()?;
                // The host's delta callback is the live consumer of a
                // streaming round; without one, or for a round the effect
                // marks non-streaming (a nested infer), the chunks drop at
                // the leaf and the completed reply is the repair.
                // Cancellation is the driver aborting this task mid-round.
                let on_delta = stream.then(|| self.ctx.on_delta().cloned()).flatten();
                Ok(tokio::spawn(async move {
                    let tool_arg = (!tools.is_empty()).then_some(tools.as_slice());
                    let result = client
                        .complete(&messages, tool_arg, &options, |delta| {
                            if let Some(hook) = &on_delta {
                                hook(delta);
                            }
                        })
                        .await
                        .map(Box::new);
                    post(&tx, id, EffectAnswer::Chat(result));
                }))
            }
            Effect::ToolCall { tool, args, .. } => {
                // Resolved by the stable identity, as a host resolves it
                // against its activated capabilities; the alias is the
                // record's, not the resolver's.
                let tool = self
                    .ctx
                    .tool_set_snapshot()?
                    .bindings()
                    .iter()
                    .find(|binding| *binding.id() == tool)
                    .map(|binding| Arc::clone(&binding.tool))
                    .ok_or(Error::internal(
                        "a tool_call effect names a tool id bound in the run's catalog",
                    ))?;
                Ok(tokio::spawn(async move {
                    let result = tool.call(args).await;
                    post(&tx, id, EffectAnswer::ToolCall(result));
                }))
            }
            Effect::UserInput { execution, section } => {
                let broker = self.ctx.input_broker().cloned().ok_or(Error::internal(
                    "a user_input effect is issued only when the run has a broker",
                ))?;
                Ok(tokio::spawn(async move {
                    let result = broker.user_input(&execution, &section).await;
                    post(&tx, id, EffectAnswer::UserInput(result));
                }))
            }
            Effect::Store { access, op } => {
                // spawn_blocking, not a plain task: the Vfs is sync by
                // design, and the blocking pool keeps a slow host-backend
                // op from stalling the driver. Aborting the handle detaches
                // rather than interrupts, so a cancelled run's in-flight op
                // completes without delivering.
                Ok(tokio::task::spawn_blocking(move || {
                    let result = run_store_op(&Store::new(&access), op);
                    // Claims-release ordering constraint: the access clone
                    // must drop after the op and before the answer posts,
                    // so the claims it holds release before a resumed chain
                    // can acquire overlapping claims; the fix changes when
                    // claims release, never whether an operation succeeds.
                    drop(access);
                    post(&tx, id, EffectAnswer::Store(result));
                }))
            }
            Effect::Timer { seconds } => {
                let duration = Duration::try_from_secs_f64(seconds).map_err(|_| {
                    Error::internal("a timer effect carries a duration Duration can hold")
                })?;
                Ok(tokio::spawn(async move {
                    tokio::time::sleep(duration).await;
                    post(&tx, id, EffectAnswer::Timer);
                }))
            }
        }
    }

    /// Posts an answer for an arbitrary effect id, so a test can drive
    /// the driver's answer paths directly.
    #[cfg(test)]
    pub(super) fn post_for_test(&self, id: EffectId, answer: EffectAnswer) {
        self.tx
            .send((id, answer))
            .expect("the scheduler holds its own receiver");
    }

    /// A clone of the answer channel's send half, so a test double can
    /// post answers from inside a performer while the driver runs.
    #[cfg(test)]
    pub(super) fn sender_for_test(&self) -> AnswerSender {
        self.tx.clone()
    }

    /// Records every performed effect's record from here on.
    #[cfg(test)]
    pub(super) fn record_effects_for_test(&mut self) -> Arc<Mutex<Vec<EffectRecord>>> {
        let tap = Arc::new(Mutex::new(Vec::new()));
        self.tap = Some(Arc::clone(&tap));
        tap
    }
}

/// Posts one answer. A send fails only when the driver is gone (a
/// cancelled run whose scheduler dropped its receiver); the answer is then
/// moot.
fn post(tx: &AnswerSender, id: EffectId, answer: EffectAnswer) {
    let _ = tx.send((id, answer));
}
