# promptforge-store

The PromptForge run-scoped virtual filesystem facade. A prompt run keeps its
bulk state in virtual files addressed by logical string paths: `Store` is a
concrete facade over a prefix-scoped VFS access capability from `shared-vfs`
(the run's `VfsRef` carries the store mount, installed by
`promptforge-vfs`'s stock constructors), exposed as `vfs.store(&access)`
through the prelude-exported `StoreExt` extension trait. Every operation is
attributed to the access's identity, so a conflicting operation by a second
live identity surfaces as `StoreError::WriteRace`.

Reads are verbatim, ranged reads slice 1-based inclusive line ranges (plain
or absolutely numbered), edits are anchor-based (`Store::str_replace`), and
glob matching is bounded and recursion-free. Every caller-supplied path is
validated into one canonical form before any backend sees it.
