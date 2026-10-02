# platform

- `dockview` imports are type-only by convention: the guard does not check import kind, so keep them `import type`.
- Framework contracts and their service tokens live here. Product contracts and their tokens stay in `crates/workshop/ui/src/services/`.
- Nothing here names a feature, a panel type, or an LED.
