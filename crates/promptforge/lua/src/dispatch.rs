//! The shared tool-dispatch body every executor invokes.
//!
//! [`dispatch_tool`] is the one place a bound tool's call composes the
//! cancel race, the per-VM call counts, the untrusted nonce wrap, and the
//! observer events. Core's model tool loop and its scheduler's `tools.call`
//! arm both call it; the agent driver adopts it unchanged. Keeping the body
//! here - the crate every executor already depends on - is what stops
//! dispatch semantics from forking.

use promptforge_api_types::cancel;
use promptforge_api_types::observe::{Observer, detail};
use promptforge_api_types::tools::OutputTrust;
use promptforge_api_types::untrusted::GuardNonce;

use crate::error::{Error, Result};
use crate::{ToolBinding, ToolCallCounts};

/// The run coordinates a script-initiated dispatch reports under.
///
/// [`dispatch_tool`] fires [`Observer::on_tool_result`] with them; a model
/// tool-loop dispatch passes `None` instead and reports the result itself,
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
/// already fired inside [`dispatch_tool`].
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

/// Dispatches one bound tool call: the shared body the executors invoke.
///
/// The sequence is fixed: the counts increment (dispatch attempted, even if
/// the tool later errors), the call raced against cancellation, the
/// succeeded/failed observation, then the trust rule - a trusted output
/// passes verbatim, anything else is nonce-wrapped before it can reach a
/// model turn or a calling script. A script-initiated call (`script` is
/// `Some`) also fires [`Observer::on_tool_result`] with the final content;
/// a model-loop call fires no content event here: the loop reports the
/// returned [`ToolDispatch`] under the model-issued call id.
///
/// # Errors
/// Returns [`Error::Interrupted`] when the run is cancelled mid-call,
/// [`Error::Tool`] when the tool itself fails (its typed error retained as
/// the cause), or the counts' own error when `binding`'s alias was never
/// seeded.
#[expect(
    clippy::too_many_arguments,
    reason = "the dispatch body names its full run coordinates in one call, exactly as the loop it was extracted from did"
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
    if let Some(counts) = counts {
        counts.increment(binding.alias())?;
    }
    // Race the tool call against cancellation so a slow or stuck tool
    // cannot hold the run past a Ctrl-C. On cancel the tool future is
    // dropped and the run ends promptly.
    let call_result = tokio::select! {
        biased;
        () = cancel::wait_cancelled() => {
            observer.observe(execution, section, detail::TOOL_CALL_FAILED);
            return Err(Error::Interrupted);
        }
        result = binding.tool().call(args) => result,
    };
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

/// Dispatches one model-issued bound tool call: the shared
/// [`dispatch_tool`] body under the model-loop failure rule, then the
/// [`Observer::on_tool_result`] report under the model's call id.
///
/// A model-issued call always resumes with content: the tool's own failure
/// ([`Error::Tool`]) becomes the call's result - the error message
/// nonce-wrapped as untrusted - so the model reads the failure and the
/// round continues; `dispatch_tool` has already fired the failed
/// observation. Cancellation, the counts increment, and every other
/// dispatch failure still propagate.
///
/// # Errors
/// Returns [`Error::Interrupted`] when the run is cancelled mid-call, or
/// the counts' own error when `binding`'s alias was never seeded.
#[expect(
    clippy::too_many_arguments,
    reason = "the model-issued body names the same run coordinates as the script body it wraps"
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
    let outcome = match dispatch_tool(
        binding, args, counts, nonce, observer, execution, section, None,
    )
    .await
    {
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

#[cfg(test)]
#[path = "dispatch-tests.rs"]
mod tests;
