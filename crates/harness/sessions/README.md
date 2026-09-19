# harness-sessions

The harness session layer: agent discovery, session state, the input wait registry, and the supervisor state machine that takes a run from alive through closing to closed. It registers the first-party capability set and is what `harness-api`'s `Session` drives. Private to the harness family; clients reach it through `harness-api`.
