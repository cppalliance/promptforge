# shared-vfs

Generic virtual filesystem machinery: the permanent bottom of the dependency stack.

- std only. No dependencies, workspace or external. The manifest test enforces this; never weaken it.
- No promptforge policy: no /_promptforge paths, no Store, no run concepts.
- The public surface is load-bearing: add defaulted methods, never change existing signatures. Every edit rebuilds the whole stack.
- Origin labels are most-specific: a section name for a chain, a tool id for a tool, a fixture name for a test - never a generic label when a specific one exists.
