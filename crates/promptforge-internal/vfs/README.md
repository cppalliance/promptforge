# promptforge-vfs

The PromptForge virtual filesystem. It holds canonical interned paths (`VfsPath`, `VfsPathBuf`), the happens-before claims ledger behind the cloneable `VfsRef` handle and the `Access` capability it vends, the mount router built through `VfsRefBuilder`, the memory and real-filesystem backends (`MemoryBackend`, `HostBackend`), and the promptforge mode policy (`ModePolicy`), the editor gate on mutations. `VfsRefBuilder::store` declares one mount as the store root, and the store view over it applies the store's strict logical-path rules; a default `VfsRef` is a memory store at `/`.

The crate is std only, with no workspace or external dependencies, and its own manifest test fails if any dependency table gains an entry. It sits at the permanent bottom of the PromptForge dependency stack.

The real-filesystem backend resolves a path two ways. Operations on a path itself (`remove`, `exists`, `stat`, `mkdir`, `rename`) contain the parent under the root and act on a final-component link as a link, never its target. Operations on contents (`read`, `read_range`, `write`, `append`, `list`, `glob`, `copy`) follow links under the containment check, which denies a link that resolves outside the root. Content operations refuse a path that passes through a dangling symbolic link.
