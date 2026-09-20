//! The shared tool-dispatch body every executor invokes.
//!
//! [`prepare_dispatch`] is the one place a bound tool's answer composes the
//! per-VM call counts, the succeeded/failed observation, the untrusted nonce
//! wrap, and the `ToolResult` report. It is synchronous and takes the tool's
//! outcome as a value, so a host that performed the call elsewhere applies
//! the same rules when the answer arrives; [`prepare_model_dispatch`] is
//! the same body under the model-issued rule (a tool's own failure resumes
//! as untrusted failure text, and the `ToolResult` fires under the model's
//! call id). Nothing here performs a call: the executor issues the call as
//! an effect, a host performs it, and one of the two bodies applies the
//! rules when the answer lands. Keeping both bodies here - the crate every
//! executor already depends on - is what stops dispatch semantics from
//! forking.

use promptforge_api_types::emitter::Emitter;
use promptforge_api_types::event::lifecycle;
use promptforge_api_types::tools::{OutputTrust, ToolError, ToolOutput};
use promptforge_api_types::untrusted::GuardNonce;

use crate::error::{Error, Result};
use crate::{ToolBinding, ToolCallCounts};

/// The run coordinates a script-initiated dispatch reports under.
///
/// [`prepare_dispatch`] fires the `ToolResult` event with them; a
/// model tool-loop dispatch passes `None` instead and reports the result itself,
/// because it owns the model-issued call id the script path lacks. A script
/// call carries no model-issued call id, so the report's `tool_call_id` is
/// empty. The chain and depth are not part of the report: the emitter
/// stamps every event with its provenance, which is what a host groups by.
#[derive(Debug, Clone, Copy)]
pub struct ScriptReport {
    /// The section's completed model-turn count at dispatch.
    pub turn: u32,
}

/// The run coordinates a model-issued dispatch reports under: the chain's
/// script coordinates plus the call id the model issued.
///
/// [`prepare_model_dispatch`] fires the `ToolResult` event under
/// `call_id`, so a host transcript correlates the result with the
/// assistant tool-call record that requested it.
#[derive(Debug, Clone)]
pub struct ModelReport {
    /// The turn the call fired in.
    pub script: ScriptReport,
    /// The model-issued call id the result answers.
    pub call_id: String,
}

/// The resolved outcome of one dispatched tool call: the final content -
/// nonce-wrapped when untrusted - beside its trust marking. A model
/// tool-loop dispatch reports the pair through the `ToolResult` event
/// itself; a script-initiated dispatch reads only the content, its report
/// already fired inside [`prepare_dispatch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDispatch {
    content: String,
    trust: OutputTrust,
}

impl ToolDispatch {
    /// The final content: trusted output verbatim, anything else
    /// nonce-wrapped.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// The trust marking the content carries: [`OutputTrust::Trusted`]
    /// for verbatim output, [`OutputTrust::Untrusted`] for the
    /// nonce-wrapped envelope.
    #[must_use]
    pub fn trust(&self) -> OutputTrust {
        self.trust
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
/// host that counted the attempt at dispatch passes `None`), the
/// succeeded/failed observation,
/// then the trust rule - a trusted output passes verbatim, anything else is
/// nonce-wrapped before it can reach a model turn or a calling script. A
/// script-initiated call (`script` is `Some`) also fires
/// the `ToolResult` event with the final content; a model-loop call
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
pub fn prepare_dispatch(
    binding: &ToolBinding,
    call_result: std::result::Result<ToolOutput, ToolError>,
    counts: Option<&ToolCallCounts>,
    nonce: &GuardNonce,
    emitter: &Emitter,
    section: &str,
    script: Option<ScriptReport>,
) -> Result<ToolDispatch> {
    if let Some(counts) = counts {
        counts.increment(binding.alias())?;
    }
    emitter.report(
        section,
        if call_result.is_ok() {
            lifecycle::TOOL_CALL_SUCCEEDED
        } else {
            lifecycle::TOOL_CALL_FAILED
        },
    );
    let output = call_result.map_err(Error::tool)?;
    // Trust travels with the output: an untrusted result is nonce-wrapped
    // before it can reach the next model turn or the calling script. Every
    // wrap in the run shares the run's nonce, so identical content yields a
    // byte-identical envelope and KV-cache prefixes stay shared across
    // rounds and fanout arms; the `<`-escaping is what actually blocks a
    // forged close tag, so the reuse costs nothing.
    let (content, trust) = match output.trust() {
        OutputTrust::Trusted => (output.text().to_owned(), OutputTrust::Trusted),
        // `OutputTrust` is `#[non_exhaustive]` in the contract crate: an
        // unknown future variant takes the safe path and is nonce-wrapped
        // as untrusted.
        _ => (nonce.wrap(output.text()), OutputTrust::Untrusted),
    };
    if let Some(report) = script {
        emitter.tool_result(section, report.turn, "", binding.alias(), &content, trust);
    }
    Ok(ToolDispatch { content, trust })
}

/// Applies the model-issued dispatch rules to one bound tool call's answer:
/// [`prepare_dispatch`] under the model-loop failure rule, then the
/// the `ToolResult` event report under the model's call id.
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
pub fn prepare_model_dispatch(
    binding: &ToolBinding,
    call_result: std::result::Result<ToolOutput, ToolError>,
    counts: Option<&ToolCallCounts>,
    nonce: &GuardNonce,
    emitter: &Emitter,
    section: &str,
    report: &ModelReport,
) -> Result<ToolDispatch> {
    let outcome =
        match prepare_dispatch(binding, call_result, counts, nonce, emitter, section, None) {
            Ok(outcome) => outcome,
            Err(Error::Tool { message, .. }) => ToolDispatch {
                content: nonce.wrap(&message),
                trust: OutputTrust::Untrusted,
            },
            Err(error) => return Err(error),
        };
    emitter.tool_result(
        section,
        report.script.turn,
        &report.call_id,
        binding.alias(),
        &outcome.content,
        outcome.trust,
    );
    Ok(outcome)
}

#[cfg(test)]
#[path = "dispatch-tests.rs"]
mod tests;
