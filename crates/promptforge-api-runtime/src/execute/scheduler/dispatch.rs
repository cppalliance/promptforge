//! The request arms: one dispatch per validated protocol request from a
//! suspended chain. Every store operation is a leaf yield, answered on the
//! blocking pool uniformly for all backends - no inline fast path - so
//! interleaving behavior never depends on which backend serves the mount.
//! A received `mcp` request is the protocol's typed reserved error.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::execute::protocol::{Answer, Request, StoreOp, ToolCallOutcome};
use crate::execute::support::MAX_CALL_DEPTH;
use crate::execute::tools::infer_round;
use crate::input::{INPUT_UNAVAILABLE_FALLBACK, InputOutcome};
use crate::lua::{
    ScriptReport, UserInputOutcome, current_tool_bindings, dispatch_tool, resolve_model_binding,
    run_store_op,
};
use crate::model::ModelBinding;
use crate::observe::{Observation, detail};
use crate::store::{Store, StoreError};
use crate::{Error, Result, cancel};

use super::{ChainId, RequestId, Scheduler};

/// The succeeded/failed observation pair one store operation reports,
/// matching the legacy direct closures event for event; `exists` reported
/// nothing there and reports nothing here.
fn store_observations(op: &StoreOp) -> Option<(Observation, Observation)> {
    let pair = match op {
        StoreOp::Write { .. } => (detail::STORE_WRITE_SUCCEEDED, detail::STORE_WRITE_FAILED),
        StoreOp::Append { .. } => (detail::STORE_APPEND_SUCCEEDED, detail::STORE_APPEND_FAILED),
        StoreOp::Read { .. } => (detail::STORE_READ_SUCCEEDED, detail::STORE_READ_FAILED),
        StoreOp::ReadNumbered { .. } => (
            detail::STORE_READ_NUMBERED_SUCCEEDED,
            detail::STORE_READ_NUMBERED_FAILED,
        ),
        StoreOp::StrReplace { .. } => (
            detail::STORE_REPLACE_SUCCEEDED,
            detail::STORE_REPLACE_FAILED,
        ),
        StoreOp::Delete { .. } => (detail::STORE_DELETE_SUCCEEDED, detail::STORE_DELETE_FAILED),
        StoreOp::Glob { .. } => (detail::STORE_GLOB_SUCCEEDED, detail::STORE_GLOB_FAILED),
        StoreOp::Exists { .. } => return None,
    };
    Some(pair)
}

/// Classifies one store operation's failure for the answer channel. A
/// claims-model conflict becomes the fatal determinism violation: the
/// driver intercepts it at the answer boundary and ends the run on the
/// spot rather than resuming it into Lua, so no author `pcall` can catch
/// it. Every other failure rides back as the call's answer carrying the
/// store's own message, exactly as the legacy closure's external error
/// surfaced at the call site (and classified `Lua` if it aborts the chunk
/// uncaught, exactly as then).
fn classify_store_failure(error: &StoreError) -> Error {
    if let Some(detail) = error.conflict_detail() {
        return Error::Determinism(detail.to_owned());
    }
    Error::Lua(error.to_string())
}

impl Scheduler<'_> {
    /// Dispatches one validated request from a suspended chain.
    ///
    /// # Errors
    /// Returns the typed protocol error for a received `mcp` request, which
    /// no call surface produces yet, or a `models.loop` cancellation, which
    /// fails the run rather than resuming into the caller.
    pub(super) async fn dispatch(&mut self, id: ChainId, request: Request) -> Result<()> {
        match request {
            Request::Infer { prompt, binding } => {
                self.dispatch_infer(id, prompt, binding);
                Ok(())
            }
            Request::Call { target, input, var } => {
                self.dispatch_call(id, &target, input.as_deref(), &var);
                Ok(())
            }
            Request::Fanout { worker, items, var } => {
                self.dispatch_fanout(id, &worker, &items, &var);
                Ok(())
            }
            Request::ToolCall { alias, args } => {
                self.dispatch_tool_call(id, &alias, args);
                Ok(())
            }
            Request::Loop {
                messages,
                messages_key,
                binding,
                compactor,
            } => {
                self.dispatch_loop(id, binding, messages, messages_key, compactor)
                    .await
            }
            Request::UserInput => {
                self.dispatch_user_input(id);
                Ok(())
            }
            Request::Store { op } => self.dispatch_store(id, op),
            // Unreachable: no section VM installs the models.chat shim, and
            // stripped coroutines make a hand-rolled yield fail validation
            // before dispatch - the mirror of the agent driver's guards for
            // the section-only requests.
            Request::Chat { .. } => Err(Error::internal(
                "a section VM cannot yield a chat request: the models.chat shim is never installed",
            )),
            Request::Mcp { .. } => Err(Error::from(Request::mcp_reserved())),
        }
    }

    /// Dispatches an `infer` request: resolves the binding and the chain's
    /// client, spawns the single gateway round onto the answer channel, and
    /// parks the chain in the pending table. A resolution failure is the
    /// call's answer, resumed into the caller so an author `pcall` can catch
    /// it exactly as on the legacy callback path.
    fn dispatch_infer(&mut self, id: ChainId, prompt: String, binding: Option<ModelBinding>) {
        match self.prepare_infer(id, prompt, binding) {
            Ok((request_id, task)) => {
                self.io_tasks.insert(request_id, task);
                self.pending.insert(request_id, id);
            }
            Err(error) => {
                self.chains[id.index()].incoming = Some(Answer::Infer(Err(error)));
                self.ready.push_back(id);
            }
        }
    }

    /// The fallible half of infer dispatch: the binding resolution (the
    /// handle's frozen binding, else the section's current model), the lazy
    /// client resolution, and the spawned round.
    fn prepare_infer(
        &mut self,
        id: ChainId,
        prompt: String,
        binding: Option<ModelBinding>,
    ) -> Result<(RequestId, tokio::task::JoinHandle<()>)> {
        let chain = &mut self.chains[id.index()];
        let binding = if let Some(binding) = binding {
            binding
        } else {
            let frame = chain
                .frame
                .as_ref()
                .ok_or(Error::internal("a live chain holds its frame"))?;
            resolve_model_binding(chain.ctx.models(), &frame.vm()?.model_runtime)?.ok_or_else(
                || Error::ModelRequired {
                    section: chain.section_name().to_owned(),
                },
            )?
        };
        if chain.client.is_none() {
            chain.client = Some(self.client.resolve()?);
        }
        let client = chain
            .client
            .as_ref()
            .ok_or(Error::internal("the client slot was just resolved"))?
            .clone();
        let observer = Arc::clone(chain.ctx.observer());
        let debug = chain.ctx.debug().cloned();
        let execution = chain.ctx.execution().to_owned();
        let section = chain.section_name().to_owned();
        let turns = Arc::clone(chain.ctx.turns());
        let request_id = RequestId(self.next_request);
        self.next_request += 1;
        let tx = self.answer_tx.clone();
        let task = tokio::spawn(async move {
            let result = infer_round(
                &client,
                &binding,
                &prompt,
                observer.as_ref(),
                debug.as_deref(),
                &execution,
                &section,
                &turns,
            )
            .await;
            // A send fails only when the driver is gone (a cancelled run);
            // the answer is then moot.
            let _ = tx.send((request_id, Answer::Infer(result)));
        });
        Ok((request_id, task))
    }

    /// Dispatches a `tool_call` request: resolves the alias against the
    /// run's full bound tool catalog, spawns the shared dispatch body onto
    /// the answer channel, and parks the chain in the pending table. Every
    /// dispatch failure - an unbound alias, the counts install - is the
    /// call's answer, resumed into the caller so an author `pcall` can
    /// catch it exactly as a tool failure.
    fn dispatch_tool_call(&mut self, id: ChainId, alias: &str, args: serde_json::Value) {
        match self.prepare_tool_call(id, alias, args) {
            Ok((request_id, task)) => {
                self.io_tasks.insert(request_id, task);
                self.pending.insert(request_id, id);
            }
            Err(error) => {
                self.chains[id.index()].incoming = Some(Answer::ToolCallResult(Err(error)));
                self.ready.push_back(id);
            }
        }
    }

    /// The fallible half of tool-call dispatch: the alias resolved against
    /// the run's full bound tool catalog (the section's effective scope
    /// shapes what the model is offered, and the author's own script is not
    /// the model, so the scope does not gate it - the model-advertised set
    /// stays section-scoped), the one-time counts install, and the spawned
    /// dispatch through the shared `dispatch_tool` body, classified by the
    /// binding's declared output kind at completion.
    fn prepare_tool_call(
        &mut self,
        id: ChainId,
        alias: &str,
        args: serde_json::Value,
    ) -> Result<(RequestId, tokio::task::JoinHandle<()>)> {
        let chain = &mut self.chains[id.index()];
        let tool_set = chain.ctx.tool_set_snapshot()?;
        let Some(binding) = tool_set.binding(alias).cloned() else {
            return Err(Error::UnboundToolCall {
                name: alias.to_owned(),
                bound: tool_set
                    .bindings()
                    .iter()
                    .map(|binding| binding.alias().to_owned())
                    .collect(),
            });
        };
        let ctx = chain.ctx.clone();
        let counts = {
            let frame = chain
                .frame
                .as_mut()
                .ok_or(Error::internal("a live chain holds its frame"))?;
            let effective = current_tool_bindings(&tool_set, &frame.vm()?.tool_runtime)?;
            frame.script_call_counts(&ctx, &effective)?
        };
        // The counts seed from the section's effective scope; a bound alias
        // outside it must still be seeded here, because the shared dispatch
        // body's increment errors on an unseeded alias.
        counts.ensure(binding.alias())?;
        let observer = Arc::clone(chain.ctx.observer());
        let execution = chain.ctx.execution().to_owned();
        let section = chain.section_name().to_owned();
        let nonce = chain.ctx.nonce().clone();
        let report = ScriptReport {
            chain_id: id.0,
            // The call depth is capped at MAX_CALL_DEPTH, far inside
            // u32; the saturation is a defensive no-op.
            depth: u32::try_from(chain.call_depth).unwrap_or(u32::MAX),
            turn: chain.ctx.turns().load(Ordering::Relaxed),
        };
        let output_kind = binding.output_kind;
        let request_id = RequestId(self.next_request);
        self.next_request += 1;
        let tx = self.answer_tx.clone();
        // A spawned task does not inherit the cancel task-local; the
        // current handle rides into the task explicitly so the shared
        // dispatch body's cancel race stays armed there. The driver also
        // aborts the task handle on cancellation, so both paths end a slow
        // tool promptly.
        let cancel = cancel::current();
        let task = tokio::spawn(async move {
            let result = cancel::maybe_scope(cancel, async {
                match dispatch_tool(
                    &binding,
                    args,
                    Some(&counts),
                    &nonce,
                    observer.as_ref(),
                    &execution,
                    &section,
                    Some(report),
                )
                .await
                {
                    Ok(outcome) => ToolCallOutcome::from_dispatch(
                        output_kind,
                        binding.alias(),
                        outcome.into_content(),
                    )
                    .map_err(Error::from),
                    Err(error) => Err(Error::from(error)),
                }
            })
            .await;
            // A send fails only when the driver is gone (a cancelled run);
            // the answer is then moot.
            let _ = tx.send((request_id, Answer::ToolCallResult(result)));
        });
        Ok((request_id, task))
    }

    /// Dispatches a `user_input` request: the run's input broker answers on
    /// a spawned task exactly as a leaf I/O round does, so a blocking wait
    /// parks its chain - the section's VM and message history intact -
    /// without blocking the driver, and cancellation aborts it through the
    /// shared in-flight abort path. With no broker configured the
    /// unavailable-fallback policy answers immediately: the fixed fallback
    /// sentence with `available` false. The wait and a delivered response
    /// are recorded through the run's observer; an unavailable answer opens
    /// no wait and records no input.
    fn dispatch_user_input(&mut self, id: ChainId) {
        let chain = &self.chains[id.index()];
        let Some(broker) = chain.ctx.input_broker().cloned() else {
            self.chains[id.index()].incoming = Some(Answer::UserInput(Ok(UserInputOutcome {
                text: INPUT_UNAVAILABLE_FALLBACK.to_owned(),
                available: false,
            })));
            self.ready.push_back(id);
            return;
        };
        let observer = Arc::clone(chain.ctx.observer());
        let execution = chain.ctx.execution().to_owned();
        let section = chain.section_name().to_owned();
        observer.observe(&execution, &section, detail::USER_INPUT_WAIT_STARTED);
        let request_id = RequestId(self.next_request);
        self.next_request += 1;
        let tx = self.answer_tx.clone();
        let task = tokio::spawn(async move {
            let answer = match broker.user_input(&execution, &section).await {
                Ok(InputOutcome::Text(text)) => {
                    observer.on_user_input(&execution, &section, &text);
                    Answer::UserInput(Ok(UserInputOutcome {
                        text,
                        available: true,
                    }))
                }
                Ok(InputOutcome::Unavailable) => Answer::UserInput(Ok(UserInputOutcome {
                    text: INPUT_UNAVAILABLE_FALLBACK.to_owned(),
                    available: false,
                })),
                Err(error) => Answer::UserInput(Err(Error::from(error))),
            };
            // A send fails only when the driver is gone (a cancelled run);
            // the answer is then moot.
            let _ = tx.send((request_id, answer));
        });
        self.io_tasks.insert(request_id, task);
        self.pending.insert(request_id, id);
    }

    /// Dispatches a `store` request: the chain's access capability runs the
    /// operation on the blocking pool and posts the answer to the channel,
    /// parking the chain in the pending table exactly as a leaf I/O round
    /// does. Every store operation takes this yield path uniformly -
    /// memory- and host-backed alike, with no inline fast path - so
    /// interleaving behavior never depends on which backend serves the
    /// mount. The operation's observation fires before the answer posts, so
    /// the event stream keeps the legacy closure path's ordering (the op's
    /// outcome precedes the chunk's closing boundary).
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the live chain's access capability
    /// is gone, which only the chain-end paths take.
    fn dispatch_store(&mut self, id: ChainId, op: StoreOp) -> Result<()> {
        let chain = &self.chains[id.index()];
        let access = Arc::clone(chain.access()?);
        let observer = Arc::clone(chain.ctx.observer());
        let execution = chain.ctx.execution().to_owned();
        let section = chain.section_name().to_owned();
        let observations = store_observations(&op);
        let request_id = RequestId(self.next_request);
        self.next_request += 1;
        let tx = self.answer_tx.clone();
        // spawn_blocking, not a plain task: the Vfs is sync by design, and
        // the blocking pool keeps a slow host-backend op from stalling the
        // driver. Aborting the handle detaches rather than interrupts, so a
        // cancelled run's in-flight op completes without delivering.
        let task = tokio::task::spawn_blocking(move || {
            let result = run_store_op(&Store::new(&access), op);
            if let Some((succeeded, failed)) = observations {
                observer.observe(
                    &execution,
                    &section,
                    if result.is_ok() { succeeded } else { failed },
                );
            }
            // Claims-release ordering constraint: the access clone must
            // drop after the op and its observation and before the answer
            // posts, so the claims it holds release before a resumed chain
            // can acquire overlapping claims; the fix changes when claims
            // release, never whether an operation succeeds.
            drop(access);
            // A send fails only when the driver is gone (a cancelled run);
            // the answer is then moot.
            let _ = tx.send((
                request_id,
                Answer::Store(result.map_err(|e| classify_store_failure(&e))),
            ));
        });
        self.io_tasks.insert(request_id, task);
        self.pending.insert(request_id, id);
        Ok(())
    }

    /// Dispatches a `call` request: constructs the child chain, pushes
    /// it on the chain stack, and enqueues it; the parent blocks until the
    /// child's finish delivers its final text as the answer. Every dispatch
    /// failure - the depth cap, target resolution, child construction - is
    /// the call's answer, resumed into the caller so an author `pcall` can
    /// catch it exactly as on the legacy callback path.
    fn dispatch_call(
        &mut self,
        id: ChainId,
        target: &str,
        input: Option<&str>,
        var: &serde_json::Value,
    ) {
        match self.prepare_call(id, target, input, var) {
            Ok(child) => {
                self.stack.push(child);
                self.ready.push_back(child);
            }
            Err(error) => {
                self.chains[id.index()].incoming = Some(Answer::Call(Err(error)));
                self.ready.push_back(id);
            }
        }
    }

    /// The fallible half of call dispatch: the depth cap checked against
    /// the caller's call-depth field, the target resolved over the
    /// caller's visible set, and the child chain constructed one level
    /// deeper under the call's args and `var` snapshot.
    fn prepare_call(
        &mut self,
        id: ChainId,
        target: &str,
        input: Option<&str>,
        var: &serde_json::Value,
    ) -> Result<ChainId> {
        let chain = &self.chains[id.index()];
        let depth = chain.call_depth + 1;
        if depth > MAX_CALL_DEPTH {
            return Err(Error::Lua(format!(
                "call recursion exceeded cap of {MAX_CALL_DEPTH}"
            )));
        }
        // An explicit input forks the chain's args (and `argv` re-derives
        // from them); a no-input call inherits the caller's context whole,
        // so the run's frozen `argv` - H1's repair included - carries into
        // the chain rather than re-deriving from the unchanged args.
        let child_ctx = match input {
            Some(input) => chain.ctx.with_args(input),
            None => chain.ctx.clone(),
        };
        let client = chain.client.clone();
        // A call chain is a blocking child: it borrows the caller's access
        // capability (the same serial thread of execution), so the caller's
        // standing claims never false-conflict with the child's ops.
        let access = chain.access.clone();
        // `chain`'s arena borrow ends here; the resolution borrows the
        // prompt tree, so the target's slice outlives it.
        let target_section = self.resolve_chain_target(id, target)?;
        let child = self.start_chain(
            child_ctx,
            target_section.slice,
            target_section.index,
            Some(id),
            var,
            depth,
            None,
        )?;
        // The child inherits the caller's client slot: an already-resolved
        // client is shared, an unresolved one stays lazy.
        self.chains[child.index()].client = client;
        self.chains[child.index()].access = access;
        Ok(child)
    }
}
