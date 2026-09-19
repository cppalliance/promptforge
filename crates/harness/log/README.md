# harness-log

The harness run log: an append-only Turso record of every run and, per run, every effect, answer, and event in loop order, indexed by task so a run can be sliced by task and ordered within one. Session transcript views, Workshop reconnect, and the `TaskEvents` performer read it; nothing reads an answer row back into the engine. Private to the harness family; clients reach it through `harness-api`.
