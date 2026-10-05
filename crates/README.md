# Crates

This directory holds the workspace's crates. The crates at this level are the public and shared layer: the Engine's public API, the Harness and the pieces a Host plugs into it, the vocabulary and discovery code the gateway shares with its clients, shared middleware and error wrappers, and the build tooling. Each product family keeps its private crates in a container directory of its own, and code in one family reaches another family only through a crate at this level.
