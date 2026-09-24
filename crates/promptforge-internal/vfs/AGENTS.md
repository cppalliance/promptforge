# promptforge-vfs

The PromptForge virtual filesystem: generic machinery and the promptforge policy over it, at the permanent bottom of the dependency stack.

- std only. No dependencies, workspace or external. The manifest test enforces this; never weaken it.
- No promptforge policy in the machinery modules: no /_promptforge paths, no Store, no run concepts. The policy (the `/_promptforge` mount layout, `empty`, and the mode gate) sits at the crate root.
- The public surface is load-bearing: add defaulted methods, never change existing signatures. Every edit rebuilds the whole stack.
- Origin labels are most-specific: a section name for a chain, a tool id for a tool, a fixture name for a test - never a generic label when a specific one exists.
