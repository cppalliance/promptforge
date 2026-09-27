# promptforge-vfs

The PromptForge virtual filesystem: generic machinery and the promptforge policy over it, at the permanent bottom of the dependency stack.

- std only. No dependencies, workspace or external. The manifest test enforces this; never weaken it.
- No promptforge policy in the machinery modules: no /_promptforge paths, no run concepts. The declared store is generic machinery - `VfsRefBuilder::store` names any mount as the store, and the store view's strict logical-path rules are the store's caller contract - so it lives with the handle; the `/_promptforge` mount layout (`STORE_MOUNT`, `empty`) and the mode gate stay at the crate root.
- The public surface is load-bearing: add defaulted methods, never change existing signatures. Every edit rebuilds the whole stack.
- Origin labels are most-specific: a section name for a chain, a tool id for a tool, a fixture name for a test - never a generic label when a specific one exists.
