//! The shared tool-dispatch body every executor invokes.
//!
//! [`prepare_dispatch`] is the one place a bound tool's answer composes the
//! per-VM call counts, the succeeded/failed observation, the untrusted nonce
//! wrap, and the `ToolResult` report. It is synchronous and takes the tool's
//! outcome as a value, so a host that performed the call elsewhere applies
//! the same rules when the answer arrives; [`prepare_model_dispatch`] is
//! the same body under the model-issued rule (a tool's own failure resumes
//! as untrusted failure text, and the `ToolResult` fires under the model's
//! call id). The executor's scheduler performs the call as an effect and
//! applies one of the two when the answer lands. [`dispatch_tool`] and
//! [`dispatch_model_tool`] are the async wrappers that still perform the
//! call here: they count the attempt, race the tool against cancellation,
//! and hand the outcome to the matching sync body. Keeping every body here -
//! the crate every executor already depends on - is what stops dispatch
//! semantics from forking.

use promptforge_api_types::cancel;
use promptforge_api_types::observe::{Observer, detail};
use promptforge_api_types::tools::{OutputTrust, ToolError, ToolOutput};
use promptforge_api_types::untrusted::GuardNonce;

use crate::error::{Error, Result};
use crate::{ToolBinding, ToolCallCounts};

/// The run coordinates a script-initiated dispatch reports under.
///
/// [`prepare_dispatch`] fires [`Observer::on_tool_result`] with them; a
/// model tool-loop dispatch passes `None` instead and reports the result itself,
/// because it owns the model-issued call id the script path lacks. A script
/// call carries no model-issued call id, so the report's `tool_call_id` is
/// empty.
#[derive(Debug, Clone, Copy)]
pub struct ScriptReport {
    /// The chain the call fired in.
    pub chain_id: u32,
    /// The calling chain's call depth.
    pub depth: u32,
    /// The section's completed model-turn count at dispatch.
    pub turn: u32,
}

/// The run coordinates a model-issued dispatch reports under: the chain's
/// script coordinates plus the call id the model issued.
///
/// [`dispatch_model_tool`] fires [`Observer::on_tool_result`] under
/// `call_id`, so a host transcript correlates the result with the
/// assistant tool-call record that requested it.
#[derive(Debug, Clone)]
pub struct ModelReport {
    /// The chain, depth, and turn the call fired in.
    pub script: ScriptReport,
    /// The model-issued call id the result answers.
    pub call_id: String,
}

/// The resolved outcome of one dispatched tool call: the final content -
/// nonce-wrapped when untrusted - beside its trust marking. A model
/// tool-loop dispatch reports the pair through [`Observer::on_tool_result`]
/// itself; a script-initiated dispatch reads only the content, its report
/// already fired inside [`prepare_dispatch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDispatch {
    content: String,
    trusted: bool,
}

impl ToolDispatch {
    /// The final content: trusted output verbatim, anything else
    /// nonce-wrapped.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Whether the tool declared its output trusted.
    #[must_use]
    pub fn trusted(&self) -> bool {
        self.trusted
    }

    /// Dissolves the outcome into its content.
    #[must_use]
    pub fn into_content(self) -> String {
        self.content
    }
}

/// Applies the dispatch rules to one bound tool call's answer: the shared
/// synchronous body every executor invokes once the tool has spoken.
///
/// The sequence is fixed: the counts increment when `counts` is `Some` (a
/// host that counts the attempt itself, as [`dispatch_tool`] does, passes
/// `None`), the succeeded/failed observation,
/// then the trust rule - a trusted output passes verbatim, anything else is
/// nonce-wrapped before it can reach a model turn or a calling script. A
/// script-initiated call (`script` is `Some`) also fires
/// [`Observer::on_tool_result`] with the final content; a model-loop call
/// fires no content event here: the loop reports the returned
/// [`ToolDispatch`] under the model-issued call id.
///
/// `call_result` is the tool's own answer, however the host obtained it.
/// Nothing here awaits, so a host that performed the call on its own
/// executor applies exactly these rules when the answer arrives.
///
/// # Errors
/// Returns [`Error::Tool`] when `call_result` is the tool's failure (its
/// typed error retained as the cause), or the counts' own error when
/// `binding`'s alias was never seeded.
#[expect(
    clippy::too_many_arguments,
    reason = "the dispatch body names its full run coordinates in one call, exactly as the loop it was extracted from did"
)]
pub fn prepare_dispatch(
    binding: &ToolBinding,
    call_result: std::result::Result<ToolOutput, ToolError>,
    counts: Option<&ToolCallCounts>,
    nonce: &GuardNonce,
    observer: &dyn Observer,
    execution: &str,
    section: &str,
    script: Option<ScriptReport>,
) -> Result<ToolDispatch> {
    if let Some(counts) = counts {
        counts.increment(binding.alias())?;
    }
    observer.observe(
        execution,
        section,
        if call_result.is_ok() {
            detail::TOOL_CALL_SUCCEEDED
        } else {
            detail::TOOL_CALL_FAILED
        },
    );
    let output = call_result.map_err(Error::tool)?;
    // Trust travels with the output: an untrusted result is nonce-wrapped
    // before it can reach the next model turn or the calling script. Every
    // wrap in the run shares the run's nonce, so identical content yields a
    // byte-identical envelope and KV-cache prefixes stay shared across
    // rounds and fanout arms; the `<`-escaping is what actually blocks a
    // forged close tag, so the reuse costs nothing.
    let (content, trusted) = match output.trust() {
        OutputTrust::Trusted => (output.text().to_owned(), true),
        // `OutputTrust` is `#[non_exhaustive]` in the contract crate: an
        // unknown future variant takes the safe path and is nonce-wrapped
        // as untrusted.
        _ => (nonce.wrap(output.text()), false),
    };
    if let Some(report) = script {
        observer.on_tool_result(
            execution,
            section,
            report.chain_id,
            report.depth,
            report.turn,
            "",
            binding.alias(),
            &content,
            trusted,
        );
    }
    Ok(ToolDispatch { content, trusted })
}

/// Applies the model-issued dispatch rules to one bound tool call's answer:
/// [`prepare_dispatch`] under the model-loop failure rule, then the
/// [`Observer::on_tool_result`] report under the model's call id.
///
/// A model-issued call always resumes with content: the tool's own failure
/// ([`Error::Tool`]) becomes the call's result - the error message
/// nonce-wrapped as untrusted - so the model reads the failure and the
/// round continues; `prepare_dispatch` has already fired the failed
/// observation. The counts increment and every other dispatch failure
/// still propagate. Nothing here awaits, so a host that performed the
/// call on its own executor applies exactly these rules when the answer
/// arrives.
///
/// # Errors
/// Returns the counts' own error when `binding`'s alias was never seeded.
#[expect(
    clippy::too_many_arguments,
    reason = "the model-issued body names the same run coordinates as the script body it wraps"
)]
pub fn prepare_model_dispatch(
    binding: &ToolBinding,
    call_result: std::result::Result<ToolOutput, ToolError>,
    counts: Option<&ToolCallCounts>,
    nonce: &GuardNonce,
    observer: &dyn Observer,
    execution: &str,
    section: &str,
    report: &ModelReport,
) -> Result<ToolDispatch> {
    let outcome = match prepare_dispatch(
        binding,
        call_result,
        counts,
        nonce,
        observer,
        execution,
        section,
        None,
    ) {
        Ok(outcome) => outcome,
        Err(Error::Tool { message, .. }) => ToolDispatch {
            content: nonce.wrap(&message),
            trusted: false,
        },
        Err(error) => return Err(error),
    };
    observer.on_tool_result(
        execution,
        section,
        report.script.chain_id,
        report.script.depth,
        report.script.turn,
        &report.call_id,
        binding.alias(),
        &outcome.content,
        outcome.trusted,
    );
    Ok(outcome)
}

/// Performs one bound tool call here: the counts increment first (a
/// dispatch attempted, as the wrappers have always counted it), so an
/// unseeded alias fails before the tool runs and a cancelled dispatch
/// still counts. The call is then raced against cancellation so a slow or
/// stuck tool cannot hold the run past a Ctrl-C; on cancel the tool
/// future is dropped, the failed observation fires, and the run ends
/// promptly.
///
/// # Errors
/// Returns the counts' own error when `binding`'s alias was never seeded,
/// or [`Error::Interrupted`] when the run is cancelled mid-call.
async fn perform_call(
    binding: &ToolBinding,
    args: serde_json::Value,
    counts: Option<&ToolCallCounts>,
    observer: &dyn Observer,
    execution: &str,
    section: &str,
) -> Result<std::result::Result<ToolOutput, ToolError>> {
    if let Some(counts) = counts {
        counts.increment(binding.alias())?;
    }
    tokio::select! {
        biased;
        () = cancel::wait_cancelled() => {
            observer.observe(execution, section, detail::TOOL_CALL_FAILED);
            Err(Error::Interrupted)
        }
        result = binding.tool().call(args) => Ok(result),
    }
}

/// Dispatches one bound tool call: performs the call here, then applies
/// [`prepare_dispatch`] to its answer.
///
/// The attempt is counted and raced against cancellation as
/// `perform_call` does. Everything else - observation, trust, the
/// `ToolResult` report - is `prepare_dispatch`'s, which is handed `None`
/// for the counts so the answer is not counted twice.
///
/// # Errors
/// Returns the counts' own error when `binding`'s alias was never seeded,
/// [`Error::Interrupted`] when the run is cancelled mid-call, or whatever
/// [`prepare_dispatch`] returns for the tool's answer.
#[expect(
    clippy::too_many_arguments,
    reason = "the async wrapper names the same run coordinates as the body it hands the answer to"
)]
pub async fn dispatch_tool(
    binding: &ToolBinding,
    args: serde_json::Value,
    counts: Option<&ToolCallCounts>,
    nonce: &GuardNonce,
    observer: &dyn Observer,
    execution: &str,
    section: &str,
    script: Option<ScriptReport>,
) -> Result<ToolDispatch> {
    let call_result = perform_call(binding, args, counts, observer, execution, section).await?;
    prepare_dispatch(
        binding,
        call_result,
        None,
        nonce,
        observer,
        execution,
        section,
        script,
    )
}

/// Dispatches one model-issued bound tool call: performs the call here,
/// then applies [`prepare_model_dispatch`] to its answer.
///
/// # Errors
/// Returns [`Error::Interrupted`] when the run is cancelled mid-call, or
/// the counts' own error when `binding`'s alias was never seeded.
#[expect(
    clippy::too_many_arguments,
    reason = "the model-issued wrapper names the same run coordinates as the body it hands the answer to"
)]
pub async fn dispatch_model_tool(
    binding: &ToolBinding,
    args: serde_json::Value,
    counts: Option<&ToolCallCounts>,
    nonce: &GuardNonce,
    observer: &dyn Observer,
    execution: &str,
    section: &str,
    report: &ModelReport,
) -> Result<ToolDispatch> {
    let call_result = perform_call(binding, args, counts, observer, execution, section).await?;
    prepare_model_dispatch(
        binding,
        call_result,
        None,
        nonce,
        observer,
        execution,
        section,
        report,
    )
}

#[cfg(test)]
#[path = "dispatch-tests.rs"]
mod tests;
