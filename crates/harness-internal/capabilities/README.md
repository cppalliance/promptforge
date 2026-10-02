# harness-capabilities

The Harness's capability layer: the `CapabilityRegistry` of installed capabilities, per-run `activate` with co-activation conflict checking and prefix-contained catalog assembly, and the `Capability` and `Tool` traits the first-party capability crates implement. The Engine binds tool slots against the descriptors activation produces and issues each call as an effect naming a tool id; the Harness resolves the id in the activation's `ToolTable` and calls the implementation here.

It also defines `InputBroker`, the part of the Host that carries a question to a person; a capability waits on it for the operator's next message. When the Host has an operator, the Harness puts its broker in the run's `RunServices`; for a Host with nobody at the other end, such as a batch or eval Host, the Harness leaves it out, and a capability reads that absence as having nobody to ask.

It holds one core capability of its own, `UserInput` (`promptforge/user-input`), because its code needs nothing beyond this crate's traits and the broker. A prompt that declares it gets an `input` table whose `input.ask()` calls the ask tool, `promptforge/user-input/ask`, by its full id; the tool waits on the run's broker, or answers a fixed fallback sentence when the run has none. A required declaration on a Host without a broker is refused.

It depends on no provider; `harness-web`, `harness-webfetch`, and `harness-web-search` depend on it for the traits, and the Harness's session runtime depends on all of them to register the first-party set. Private to the Harness family in `crates/harness-internal/`; clients reach it through the `harness` facade. Like every Harness crate, it may depend only on `promptforge` and its container siblings.
