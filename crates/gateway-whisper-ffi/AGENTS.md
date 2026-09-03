# gateway-whisper-ffi

Runtime-loaded safe bindings for the pinned whisper.cpp C ABI: library load, context and state ownership, and parameter layout for the b4938 pin.

- Unsafe is confined to this crate's FFI boundary; every `unsafe` block carries a `// SAFETY:` comment on the immediately preceding line naming the lifetime, null, and ownership invariants the call relies on.
- Raw whisper pointers live only behind Drop-owning wrappers (`ContextInner`, `WhisperState`); never expose a raw `*mut` across the safe API, and never free or clone a pointer outside those Drop impls.
- The C ABI pin (struct sizes, symbol set, b4938 layout tests) is the contract with the packaged shared library; do not widen the surface or retarget a newer whisper.cpp without updating the pin and its size assertions in the same change.
