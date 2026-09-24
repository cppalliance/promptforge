PromptForge API: the one crate a host depends on to parse prompt files and drive their runs.

A host parses a source into a [`Prompt`], prepares a [`RunContext`] through an [`Environment`], and drives the [`Run`] state machine: [`Run::step`] returns the effects to perform and the events to log, and [`Run::resume`] hands each effect's answer back. The engine performs no I/O and reads no clock; the vocabulary it exchanges with a host sits in the role modules below.

Every item here is re-exported from the engine's private crates, and each has exactly one path.
