# gateway-local

This crate owns Gateway local-inference provisioning and the managed `llama-server` child lifecycle.

- Keep artifact provisioning, verification, dialect probing, cache storage, and child-process ownership here.
- HTTP adapters, routing, profile switching, and Gateway error types stay in their owning Gateway layers.
- `gateway-stt` reuses the public `ArtifactStore` for speech-model provisioning instead of creating a second artifact store.
