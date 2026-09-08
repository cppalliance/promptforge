# promptforge-core-support

This crate holds shared host-support primitives and canonical runtime-event vocabulary.

- Everything reported through `Observer` is report-only. Reported data cannot steer an execution decision.
- Read-side history uses the separate `EventLog` input, never the report channel.
- This crate stays at the bottom of the PromptForge dependency graph and does not depend on other PromptForge crates.
- One nonce per run; identical content must produce a byte-identical run envelope.
- The control-markup inventory is closed on purpose: additive table entries with a family rationale only, never matcher generalization.
