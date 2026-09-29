The engine's filesystem types a launch names: the [`VfsRef`] a client hands a session in [`crate::LaunchOptions::vfs`], and the [`VfsError`] a store read reports through [`crate::OutputError::Store`].

A client builds the handle with the engine's public `promptforge::vfs`: a builder that declares the store and mounts host directories or its own backends beside it, overlays, a policy, and an operation sink. The harness stages the prompt's declared input file and reads its declared output file through that handle's store.
