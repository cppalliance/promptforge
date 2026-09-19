//! The `models.loop` dispatch: the Rust-backed model-tool loop runs on the
//! driver thread with the section VM behind its append sink, compactor
//! invocation, and local-tool dispatcher, then the chain resumes with the
//! loop's answer.

use std::collections::BTreeMap;

use mlua::RegistryKey;

use crate::execute::protocol::Answer;
use crate::execute::scope::prepare_effective_scope;
use crate::execute::tool_loop::run_models_loop;
use crate::lua::{
    MessageRecord, OverflowReason, append_message_record, current_tool_bindings, invoke_selected,
    project_messages, resolve_model_binding,
};
use crate::model::ModelBinding;
use crate::observe::detail;
use crate::tools::ToolId;
use crate::{Error, Result};

use super::{ChainId, Scheduler};

impl Scheduler<'_> {
    /// Dispatches a `loop` request: runs the Rust-backed model-tool loop on
    /// the driver thread, then resumes the chain with the nil answer. The
    /// loop holds the section VM through its append sink and local-tool
    /// dispatcher, so it cannot cross a spawned-task boundary; while it
    /// runs, other chains wait (a fanout arm's loop serializes its sibling
    /// arms' steps behind its rounds). Every loop failure but cancellation
    /// is the call's answer, resumed into the caller so an author `pcall`
    /// catches it exactly as on the other dispatch paths; cancellation
    /// fails the run, exactly as the agent driver treats it.
    pub(super) async fn dispatch_loop(
        &mut self,
        id: ChainId,
        binding: Option<ModelBinding>,
        messages: Vec<MessageRecord>,
        messages_key: RegistryKey,
        compactor: Option<RegistryKey>,
    ) -> Result<()> {
        // Boxed: the loop's future carries the whole dissolved frame
        // context, and the driver future must stay small (the workspace's
        // large-futures lint gates `run`).
        let outcome =
            Box::pin(self.run_loop(id, binding, &messages, &messages_key, compactor)).await;
        match outcome {
            Ok(()) => {
                self.chains[id.index()].incoming = Some(Answer::Loop(Ok(())));
                self.ready.push_back(id);
                Ok(())
            }
            Err(Error::Interrupted) => Err(Error::Interrupted),
            Err(error) => {
                self.chains[id.index()].incoming = Some(Answer::Loop(Err(error)));
                self.ready.push_back(id);
                Ok(())
            }
        }
    }

    /// The fallible half of loop dispatch: the binding resolution (the
    /// handle's frozen binding, else the section's current model), the lazy
    /// client resolution, the call-time tool scope (the effective bindings
    /// plus the section's local tools, near-duplicate checked), the
    /// one-time counts install, the per-dispatch projection, and the loop
    /// itself, run with the section VM behind the append sink, the
    /// compactor invocation, and the local-tool dispatcher.
    #[expect(
        clippy::too_many_lines,
        reason = "the preparation lifts every loop input out of the chain borrow in one linear sequence before the VM-borrowed loop phase"
    )]
    async fn run_loop(
        &mut self,
        id: ChainId,
        binding: Option<ModelBinding>,
        messages: &[MessageRecord],
        messages_key: &RegistryKey,
        compactor: Option<RegistryKey>,
    ) -> Result<()> {
        let (
            client,
            binding,
            schemas,
            dispatch,
            global_aliases,
            counts,
            mut conversation,
            execution,
            section,
            observer,
            debug,
            turns,
            nonce,
            max_iterations,
            on_delta,
        ) = {
            let chain = &mut self.chains[id.index()];
            let execution = chain.ctx.execution().to_owned();
            let section = chain.section_name().to_owned();
            let binding = if let Some(binding) = binding {
                binding
            } else {
                let frame = chain
                    .frame
                    .as_ref()
                    .ok_or(Error::internal("a live chain holds its frame"))?;
                resolve_model_binding(chain.ctx.models(), &frame.vm()?.model_runtime)?.ok_or_else(
                    || Error::ModelRequired {
                        section: section.clone(),
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
            let tool_set = chain.ctx.tool_set_snapshot()?;
            let max_iterations = chain.ctx.max_tool_iterations();
            let nonce = chain.ctx.nonce().clone();
            let on_delta = chain.ctx.on_delta().cloned();
            let frame = chain
                .frame
                .as_mut()
                .ok_or(Error::internal("a live chain holds its frame"))?;
            // The scope is read at call time: `tools.add` and
            // `tools.add_local` calls since the last model operation shape
            // this call's advertised set.
            let effective = current_tool_bindings(&tool_set, &frame.vm()?.tool_runtime)?;
            let handles = frame.reporting_handles();
            let observer = handles.observer;
            let debug = handles.debug;
            let turns = handles.turns;
            let counts = frame.script_call_counts(&chain.ctx, &effective)?;
            let local_schemas = frame.vm()?.local_tool_schemas()?;
            // The shared dispatch body's increment errors on an unseeded
            // alias, so the local aliases seed alongside the bound scope.
            for schema in &local_schemas {
                counts.ensure(&schema.name)?;
            }
            let (schemas, dispatch) = prepare_effective_scope(
                &effective,
                &local_schemas,
                &execution,
                observer.as_ref(),
                &section,
            )?;
            let global_aliases: BTreeMap<String, ToolId> = tool_set
                .bindings()
                .iter()
                .map(|binding| (binding.alias().to_owned(), binding.id().clone()))
                .collect();
            // The per-dispatch projection, over the list as the author
            // holds it now. A projection failure reports a failed turn
            // before its call-site error resumes into Lua, the agent
            // driver's precedent.
            let conversation = match project_messages(messages) {
                Ok(conversation) => conversation,
                Err(error) => {
                    observer.observe(&execution, &section, detail::MODEL_TURN_FAILED);
                    return Err(Error::from(error));
                }
            };
            (
                client,
                binding,
                schemas,
                dispatch,
                global_aliases,
                counts,
                conversation,
                execution,
                section,
                observer,
                debug,
                turns,
                nonce,
                max_iterations,
                on_delta,
            )
        };
        let completion_options = binding.completion_options();
        let context = binding.context();
        let chain = &self.chains[id.index()];
        let frame = chain
            .frame
            .as_ref()
            .ok_or(Error::internal("a live chain holds its frame"))?;
        let vm = frame.vm()?;
        // The append sink: every assistant message and correlated tool
        // result lands in the author's own message list as its round
        // completes.
        let mut append = |record: &MessageRecord| -> Result<()> {
            append_message_record(vm.lua(), messages_key, record).map_err(Error::from)
        };
        // The selected compactor, invoked with the overflow reason: the
        // omitted default is `compactors.fail`; an explicit callback runs
        // on this VM and its typed raise crosses back downcastable.
        let invoke = |reason: OverflowReason| -> Error {
            invoke_selected(vm.lua(), compactor.as_ref(), reason).into()
        };
        // Local tools are Lua functions on this section VM; route their
        // calls back into it rather than the bound dispatch body.
        let local = |alias: &str, args: serde_json::Value| -> Result<String> {
            vm.call_local_tool(alias, &args).map_err(Error::from)
        };
        run_models_loop(
            &client,
            &schemas,
            &dispatch,
            &mut conversation,
            &mut append,
            max_iterations,
            context,
            &invoke,
            &execution,
            observer.as_ref(),
            &section,
            &turns,
            debug.as_deref(),
            &completion_options,
            &nonce,
            Some(&counts),
            Some(&global_aliases),
            Some(&local),
            on_delta.as_deref(),
        )
        .await
    }
}
