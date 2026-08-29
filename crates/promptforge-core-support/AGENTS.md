# promptforge-core-support

This crate holds small shared host-support primitives: untrusted-data guard
wrapping (`untrusted`), cooperative cancellation (`cancel`), and report-only
run observation (`observe`).

## Rules

- Small shared host-support primitives only: untrusted guards, cancellation,
  observation. No dependencies on other promptforge crates - every
  promptforge crate may depend on this one, so this one depends on none of
  them.
- The observation vocabulary is report-only: nothing here may be read back
  to steer an execution decision.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
