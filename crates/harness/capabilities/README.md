# harness-capabilities

The harness's capability layer: the registry, activation with co-activation conflict checking, and the `Capability`, `Tool`, and `InputBroker` traits. It depends on no provider; the first-party capability crates depend on it for the traits, and `harness-sessions` depends on all of them to register the first-party set. Private to the harness family; clients reach it through `harness-api`.
