# gateway-config

Typed, validated configuration for the PromptForge gateway.

## Rules

- Declarative configuration only. This crate parses, interpolates, and
  validates operator-supplied TOML. It never performs network I/O and never
  executes a process; artifact download, verification, and launch belong to
  the gateway.
- Diagnostics are secret-safe. Error messages may name fields, model names,
  and sources, but never render a `Secret` or credential material.
- Validation happens before values leave the crate. A `Config` (and every
  companion type in it) cannot be constructed without passing validation, so
  downstream code never re-checks or clamps operator input. New fields are
  rejected at deserialize or validate time, never silently ignored.
- New local-model companion types live in `src/config/companion.rs`; do not
  expand `src/config/accessors.rs` for them.
