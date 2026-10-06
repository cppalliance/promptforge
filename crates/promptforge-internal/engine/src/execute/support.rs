//! Cross-cutting run helpers: the turn counter, the `sys` JSON, the shared
//! round report, and the shared run constants.

use std::sync::atomic::{AtomicU32, Ordering};

use promptforge_model_client::detail::{completion_into_result, completion_metadata_diagnostics};
use promptforge_types::emitter::Emitter;
use promptforge_types::event::lifecycle;
use promptforge_types::event::{Event, ReplyOrigin};
use promptforge_types::ids::RoundId;
use promptforge_types::metrics::CallMetrics;

use crate::model::{Completion, CompletionResult};

/// Maximum nested `call()` depth (inclusive of the first call).
pub(super) const MAX_CALL_DEPTH: usize = 8;

/// The run's final result when no section produced a reply: the generic
/// completion text both fallback sites (an empty walk, an H1-only run) share.
pub(super) const GENERIC_COMPLETION: &str = "done";

/// Advances the shared turn counter with saturation, returning the 1-based
/// index of the turn just started.
///
/// Uses `try_update` so the STORED counter saturates at [`u32::MAX`] rather
/// than wrapping through `fetch_add`. A wrapped counter would reuse a turn index
/// and desynchronize debug capture; saturation makes that unrepresentable. The
/// closure never returns `None`, so the update never fails.
pub(super) fn advance_turn(turns: &AtomicU32) -> u32 {
    turns
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_add(1))
        })
        .unwrap_or(u32::MAX)
        .saturating_add(1)
}

/// The `sys` JSON every section driver builds for its section or arm: the
/// six shared fields in one construction. A driver with an extra field
/// (`index`, on a fanout arm or a spawned chain) inserts it at its own call
/// site. `when` is the run's `started_at` rendered as RFC 3339, the same
/// string in every section; `id` is the section entry's hierarchical id
/// (the entering chain's id extended by its local entry counter), rendered
/// as a dot-separated path; `taskid` is the id of the nearest enclosing
/// task (the main walk is task `0`; a `call` child reports its caller's
/// task), the handle a chain passes to `tasks.*` to speak about itself.
pub(super) fn sys_json(
    when: &str,
    id: &str,
    task_id: &str,
    section_name: &str,
    execution: &str,
    section_count: usize,
) -> serde_json::Value {
    serde_json::json!({
        "when": when,
        "id": id,
        "taskid": task_id,
        "section_name": section_name,
        "execution": execution,
        "section_count": section_count,
    })
}

/// What a served completion reports once the turn has advanced and the
/// round-level events have fired: the pieces the chat arm's answer arms
/// carry alongside the outcome. The nested-inference arm ignores it.
pub(super) struct Served {
    pub(super) finish_reason: Option<String>,
    pub(super) model: String,
    pub(super) metrics: Option<CallMetrics>,
}

/// Reports one served completion's round-level events and returns its
/// outcome beside the metadata an answer needs.
///
/// The sequence is fixed and lives here for both the chat and the nested
/// inference paths: the debug request/response pair (when debug capture is
/// on, from the completion's raw exchange), `MODEL_TURN_COMPLETED`,
/// one `model_metadata_degraded` per metadata diagnostic the completion
/// holds, the thinking side channel, the `length` truncation observation,
/// then exactly one `assistant_reply` content report carrying `origin` -
/// the chat arm's [`ReplyOrigin::Chat`] or the nested-inference arm's
/// [`ReplyOrigin::Infer`]. A non-text outcome fires the shared prefix and
/// no content report. The thinking and the reply carry `round`, the id
/// the round's `Chat` effect held, and name the model the completion
/// names.
pub(super) fn report_model_turn(
    emitter: &Emitter,
    section: &str,
    turn: u32,
    round: RoundId,
    completion: Completion,
    origin: ReplyOrigin,
) -> (CompletionResult, Served) {
    let metrics = completion.metrics().cloned();
    let model = completion.model().to_owned();
    let thinking = completion
        .reasoning_content()
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let finish_reason = completion.finish_reason().map(str::to_owned);
    if emitter.captures_debug() {
        // A completion no broker attached a raw exchange to still reports
        // its pair, with `null` bodies, so the caller can pair every round.
        let (request, response) = completion
            .raw()
            .map_or((serde_json::Value::Null, serde_json::Value::Null), |raw| {
                (raw.request().clone(), raw.response().clone())
            });
        emitter.request(section, turn, request);
        emitter.response(
            section,
            turn,
            response,
            finish_reason.clone(),
            completion.reasoning_content().map(str::to_owned),
        );
    }
    emitter.report(section, lifecycle::MODEL_TURN_COMPLETED);
    for message in completion_metadata_diagnostics(&completion) {
        emitter.emit(section, |execution, section, provenance| {
            Event::ModelMetadataDegraded {
                execution,
                section,
                provenance,
                turn,
                message: message.clone(),
            }
        });
    }
    // The content reports a transcript is built from: the
    // thinking side channel first, then the reply, each with the round,
    // the model, and the metrics.
    if let Some(thinking) = &thinking {
        emitter.thinking(section, turn, round, &model, thinking);
    }
    let served = Served {
        finish_reason,
        model,
        metrics,
    };
    let outcome = completion_into_result(completion);
    if let CompletionResult::Text(text) = &outcome {
        if served.finish_reason.as_deref() == Some("length") {
            emitter.report(section, lifecycle::MODEL_TURN_TRUNCATED);
        }
        let finish_reason = served.finish_reason.as_deref();
        let metrics = served.metrics.as_ref();
        emitter.assistant_reply(
            section,
            turn,
            round,
            text,
            finish_reason,
            &served.model,
            metrics,
            origin,
        );
    }
    (outcome, served)
}

#[cfg(test)]
#[path = "support-tests.rs"]
mod tests;
