# gateway-whisper-ffi

This crate owns runtime-loaded safe bindings for the packaged whisper.cpp C ABI.

- Unsafe is confined to this crate's FFI boundary; every `unsafe` block carries a `// SAFETY:` comment on the immediately preceding line naming the lifetime, null, and ownership invariants the call relies on.
- Raw whisper pointers live only behind Drop-owning wrappers (`ContextInner`, `WhisperState`); never expose a raw `*mut` across the safe API, and never free or clone a pointer outside those Drop impls.
- The packaged C ABI pin covers struct sizes, the symbol set, and parameter layout. A retarget must update the pin and its size assertions in the same change.
