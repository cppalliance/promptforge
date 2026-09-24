//! Cross-cutting run helpers: the turn counter, the `sys` JSON, the round
//! metrics, the shared round report, and the shared run constants.

use std::sync::atomic::{AtomicU32, Ordering};

use promptforge_model_client::detail::{
    completion_into_result, completion_take_request_body, completion_take_response_body,
    completion_vllm_metrics,
};
use promptforge_types::emitter::Emitter;
use promptforge_types::event::ReplyOrigin;
use promptforge_types::event::lifecycle;
use promptforge_types::metrics::CallMetrics;

use crate::model::{Completion, CompletionResult};

/// Maximum nested `call()` depth (inclusive of the first call).
pub(crate) const MAX_CALL_DEPTH: usize = 8;

/// The run's final result when no section produced a reply: the generic
/// completion text both fallback sites (an empty walk, an H1-only run) share.
pub(crate) const GENERIC_COMPLETION: &str = "done";

/// Advances the shared turn counter with saturation, returning the 1-based
/// index of the turn just started.
///
/// Uses `fetch_update` so the STORED counter saturates at [`u32::MAX`] rather
/// than wrapping through `fetch_add`. A wrapped counter would reuse a turn index
/// and desynchronize debug capture; saturation makes that unrepresentable. The
/// closure never returns `None`, so the update never fails.
pub(crate) fn advance_turn(turns: &AtomicU32) -> u32 {
    turns
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_add(1))
        })
        .unwrap_or(u32::MAX)
        .saturating_add(1)
}

/// The `sys` JSON every engine driver builds for its section or arm: the
/// six shared fields in one construction. A driver with an extra field
/// (`index`, on a fanout arm or a spawned chain) inserts it at its own call
/// site. `when` is the run's `started_at` rendered as RFC 3339, the same
/// string in every section; `id` is the section entry's hierarchical id
/// (the entering chain's id extended by its local entry counter), rendered
/// as a dot-separated path; `taskid` is the id of the nearest enclosing
/// task (the main walk is task `0`; a `call` child reports its caller's
/// task), the handle a chain passes to `tasks.*` to speak about itself.
pub(crate) fn sys_json(
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

/// Assembles one round's [`CallMetrics`] from everything the completion
/// measured, or `None` when nothing was measured.
pub(crate) fn call_metrics(completion: &Completion) -> Option<CallMetrics> {
    let metrics = CallMetrics {
        usage: completion.usage().cloned(),
        llama: completion.llama_timings().cloned(),
        vllm: completion_vllm_metrics(completion).cloned(),
        client: completion.client_timing().cloned(),
    };
    let measured = metrics.usage.is_some()
        || metrics.llama.is_some()
        || metrics.vllm.is_some()
        || metrics.client.is_some();
    measured.then_some(metrics)
}

/// What a served completion reports once the turn has advanced and the
/// round-level events have fired: the pieces the chat arm's answer arms
/// carry alongside the outcome. The nested-inference arm ignores it.
pub(crate) struct Served {
    pub(crate) finish_reason: Option<String>,
    pub(crate) model: String,
    pub(crate) metrics: Option<CallMetrics>,
}

/// Reports one served completion's round-level events and returns its
/// outcome beside the metadata an answer needs.
///
/// The sequence is fixed and lives here for both the chat and the nested
/// inference paths: the debug request/response pair, `MODEL_TURN_COMPLETED`,
/// the thinking side channel, the `length` truncation observation, then
/// exactly one `assistant_reply` content report carrying `origin` - the
/// chat arm's [`ReplyOrigin::Chat`] or the nested-inference arm's
/// [`ReplyOrigin::Infer`]. A non-text outcome fires the shared prefix and
/// no content report.
pub(crate) fn report_model_turn(
    emitter: &Emitter,
    section: &str,
    turn: u32,
    mut completion: Completion,
    origin: ReplyOrigin,
) -> (CompletionResult, Served) {
    let metrics = call_metrics(&completion);
    let model = completion.model().to_owned();
    let thinking = completion
        .reasoning_content()
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let finish_reason = completion.finish_reason().map(str::to_owned);
    if emitter.captures_debug() {
        emitter.request(section, turn, completion_take_request_body(&mut completion));
        emitter.response(
            section,
            turn,
            completion_take_response_body(&mut completion),
            finish_reason.clone(),
            completion.reasoning_content().map(str::to_owned),
        );
    }
    emitter.report(section, lifecycle::MODEL_TURN_COMPLETED);
    // The content reports every host transcript is built from: the
    // thinking side channel first, then the reply, each with model and
    // metrics.
    if let Some(thinking) = &thinking {
        emitter.thinking(section, turn, &model, thinking);
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
            text,
            finish_reason,
            &served.model,
            metrics,
            origin,
        );
    }
    (outcome, served)
}
