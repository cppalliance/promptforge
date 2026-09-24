# promptforge-types

This crate holds shared host-support primitives and canonical runtime-event vocabulary.

- Every `Event` the engine returns is report-only; reported data cannot steer an execution decision.
- Read-side history is requested through the `TaskEvents` effect and answered by the host; the engine never reads back the events it returned.
- This crate stays at the bottom of the PromptForge dependency graph and does not depend on other PromptForge crates, apart from the doctest-only `promptforge` dev-dependency the root `AGENTS.md` excepts: its doc examples alone compile against the facade, and no unit test, integration test, or bench imports it.
- One nonce per run; identical content must produce a byte-identical run envelope.
- The control-markup inventory is closed on purpose: additive table entries with a family rationale only, never matcher generalization.
