# promptforge-store

The PromptForge run-scoped virtual filesystem. A prompt run keeps its bulk
state in virtual files addressed by logical string paths: `Store` is the
backend contract, `MemStore` and `FileStore` are the in-memory and
filesystem backends, and `StoreRef` is the cheaply cloneable, thread-safe
handle the runtime shares between the Lua VM and the model's file tools.

Reads are verbatim, ranged reads slice 1-based inclusive line ranges (plain
or absolutely numbered), edits are anchor-based (`Store::str_replace`), and
glob matching is bounded and recursion-free. Every caller-supplied path is
validated into one canonical form before any backend sees it.
