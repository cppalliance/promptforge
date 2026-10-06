# promptforge-engine

- Concrete providers stay in their provider crates. The Engine may re-export them for crate-internal use but never reacquires provider implementation.
- Store access is decided only by the executor: every store view is derived from the chain's access inside the Engine at dispatch; the caller, performing a `Vfs` effect, uses the view it was given and never derives, widens, or retains store scope from it.
