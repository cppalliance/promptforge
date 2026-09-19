//! Cross-cutting run helpers: the turn counter, the `sys` JSON, and the
//! shared run constants.

use std::sync::atomic::{AtomicU32, Ordering};

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
/// string in every section because the engine reads no clock; `id` is the
/// section entry's hierarchical id (the entering chain's id extended by its
/// local entry counter), rendered as a dot-separated path; `taskid` is the
/// id of the nearest enclosing task (the main walk is task `0`; a `call`
/// child reports its caller's task), the handle a chain passes to `tasks.*`
/// to speak about itself.
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
