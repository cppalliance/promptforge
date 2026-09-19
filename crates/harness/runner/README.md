# harness-runner

The harness effect loop. It steps an engine `Run`, performs each effect on tokio through one performer per effect kind, feeds the answers back, and owns cancellation and supervision. Every tokio task the harness spawns goes through this crate's `spawn` module, which tags the task with its provenance so a run's tasks can be traced as a group. Private to the harness family; clients reach it through `harness-api`.
