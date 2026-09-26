# harness-capabilities

The harness's capability layer: the `CapabilityRegistry` of installed capabilities, per-run `activate` with co-activation conflict checking and prefix-contained catalog assembly, and the `Capability` and `Tool` traits the first-party capability crates implement. The engine binds tool slots against the descriptors activation produces and issues each call as an effect naming a tool id; the harness resolves the id in the activation's `ToolTable` and calls the implementation here.

It depends on no provider; `harness-web`, `harness-webfetch`, and `harness-web-search` depend on it for the traits, and the harness's session runtime depends on all of them to register the first-party set. Private to the harness family; clients reach it through `harness-api`.
