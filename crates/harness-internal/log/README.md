# harness-log

The Harness run log: an append-only Turso record of every run and, per run, every effect, answer, and event in loop order. Session transcript views and Workshop reconnect read it; nothing reads an answer row back into the Engine. Private to the Harness family in `crates/harness-internal/`; clients reach it through the `harness` facade. Like every Harness crate, it may depend only on `promptforge` and its container siblings.
