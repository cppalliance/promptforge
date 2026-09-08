# promptforge-tools

This crate contains runtime-agnostic tool vocabulary only.

- It never depends on transport clients, concrete providers, Lua, a parser, an executor, or a product crate.
- Concrete tool implementations live in provider crates that depend on this vocabulary.
