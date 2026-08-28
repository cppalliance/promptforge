//! Cross-cutting run helpers: the turn counter, the checked timestamp, and
//! the shared run constants.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::{Error, Result};

/// Maximum nested `execute()` depth (inclusive of the first call).
pub(crate) const MAX_EXECUTE_DEPTH: usize = 8;

/// The prompt language major this executor implements.
pub(crate) const SUPPORTED_MAJOR: u32 = 1;

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

/// Hands out the next run-global execution id.
///
/// H1 keeps id 0, so the counter starts at 0 and the first section or fanout
/// arm takes 1. Every section entry and every arm takes the next value, so
/// entering the same section twice yields two ids. A u64 cannot wrap in any
/// reachable run, so unlike [`advance_turn`] this needs no saturation.
pub(crate) fn next_id(ids: &AtomicU64) -> u64 {
    ids.fetch_add(1, Ordering::Relaxed) + 1
}

/// The `sys` JSON every engine driver builds for its section or arm: the six
/// shared fields in one construction. A driver with an extra field (the
/// fanout arm's `index`) inserts it at its own call site.
pub(crate) fn sys_json(
    when: &str,
    now: &str,
    id: u64,
    section_name: &str,
    execution: &str,
    section_count: usize,
) -> serde_json::Value {
    serde_json::json!({
        "when": when,
        "now": now,
        "id": id,
        "section_name": section_name,
        "execution": execution,
        "section_count": section_count,
    })
}

/// The current UTC time as an RFC 3339 string.
///
/// # Errors
/// Returns [`Error::TimestampFormat`] when the well-known RFC 3339 formatter
/// fails to render the current time, preserving the concrete
/// [`time::error::Format`] cause instead of coercing it to an empty timestamp
/// or a source-free internal error.
pub(crate) fn now_rfc3339_checked() -> Result<String> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(Error::TimestampFormat)
}
