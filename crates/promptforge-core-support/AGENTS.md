# promptforge-core-support

This crate holds small shared host-support primitives: untrusted-data guard wrapping (`untrusted`), cooperative cancellation (`cancel`), report-only run observation (`observe`), and the canonical metrics and runtime-event vocabulary with its read-side log interface (`events`).

- Everything reported through the `Observer` - the `Observation` vocabulary and the `on_*` content methods alike - is report-only: nothing reported may be read back to steer an execution decision. Read-side history is the separate `EventLog` trait alone, an explicit run input rather than a report channel, which is the split that keeps this rule satisfiable.
- Small shared host-support primitives only: untrusted guards, cancellation, observation, and the metrics/event vocabulary. No dependencies on other promptforge crates - this crate sits at the bottom of the graph so nothing cycles.
- One nonce per run; identical content must produce a byte-identical envelope (KV-cache sharing and snapshot tests depend on it).
- The control-markup inventory is closed on purpose: additive table entries with a family rationale only, never matcher generalization.
