# harness-log

The harness run log: an append-only Turso record of every run and, per run, every effect, answer, and event in loop order, indexed by task so a run can be sliced by task and ordered within one. Session transcript views, Workshop reconnect, and the `TaskEvents` performer read it; nothing reads an answer row back into the engine. Private to the harness family in `crates/harness-internal/`; clients reach it through the `harness` facade. Like every harness crate, it may depend only on `promptforge` and its container siblings.
