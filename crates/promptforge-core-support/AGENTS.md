# promptforge-core-support

This crate holds small shared host-support primitives: untrusted-data guard
wrapping (`untrusted`), cooperative cancellation (`cancel`), and report-only
run observation (`observe`).

## Rules

- Small shared host-support primitives only: untrusted guards, cancellation,
  observation. No dependencies on other promptforge crates - this crate sits
  at the bottom of the graph so nothing cycles.
- One nonce per run; identical content must produce a byte-identical envelope
  (KV-cache sharing and snapshot tests depend on it).
- The control-markup inventory is closed on purpose: additive table entries
  with a family rationale only, never matcher generalization.
- The observation vocabulary is report-only: nothing here may be read back
  to steer an execution decision.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
