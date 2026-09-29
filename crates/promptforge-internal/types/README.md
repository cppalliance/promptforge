# promptforge-types

The shared vocabulary and host-support primitives at the bottom of the PromptForge crate graph:

- `untrusted` wraps untrusted external data in a nonce-guarded envelope, and `cancel` is the polled cancellation tree the engine observes.
- `event` is the report-only `Event` vocabulary a run returns to its host, `emitter` is the provenance-stamping `Emitter` every engine crate reports through, and `metrics` is the model-call metrics vocabulary those events embed.
- `models` is the model identity and catalog vocabulary (`ModelId`, `ModelCatalog`, `ModelDescriptor`, `ThinkingMode`), and `wire` is the `StreamDelta` a host's streaming hook observes.
- `tools` is the implementation-free tool vocabulary (descriptor, catalog, trusted output, model-safe tool error), `capabilities` is the `CapabilityId` a prompt declares, and `names` is the one naming grammar both follow.
- `ids` is the deterministic identity of a run's chains and tasks and the `Provenance` replay key, `replay` holds the behavior `Flags` a run records and the replay error kinds, and `timestamp` is the UTC instant a run starts from.
- `detail` holds the unchecked identity constructors only the engine uses; the facade never re-exports it.

The crate has no normal dependency on another PromptForge crate and declares no async runtime. Its one dev-dependency, the `promptforge` facade, exists only so the doc examples compile against the paths hosts see.
