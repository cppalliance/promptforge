# platform

`@workshop/platform`, the Workshop family's browser-side UI mechanics: behavior with no visuals and no product vocabulary. `@workshop/look` is its visual sibling; neither imports the other.

- This package imports only its own files and `dockview`; `test/boundary.mjs` fails on anything else, including `@workshop/look`, the Workshop UI, and other packages. `dockview` imports are type-only by convention: the guard does not check import kind, so keep them `import type`.
- Registries are module-level singletons holding registration data, not application state. Application state lives in services with change emitters passed through constructors.
- Framework contracts and their service tokens live here. Product contracts and their tokens stay in `crates/workshop/ui/src/services/`.
- Nothing here names a feature, a panel type, or an LED.
