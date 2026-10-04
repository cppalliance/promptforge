# Virtual filesystem

This crate is PromptForge's virtual filesystem: canonical paths, the claims that order concurrent access, the mount router, memory and real-filesystem backends, the store view, and the policy gate on edits. It has no dependencies at all and sits at the bottom of the dependency stack. The real-filesystem backend keeps every path and every followed link inside its root.
