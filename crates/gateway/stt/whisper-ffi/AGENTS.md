# gateway-whisper-ffi

- Unsafe is confined to this crate's FFI boundary.
- Raw whisper pointers live only behind Drop-owning wrappers (`ContextInner`, `WhisperState`); never expose a raw `*mut` across the safe API, and never free or clone a pointer outside those Drop impls.
- The packaged C ABI pin covers struct sizes, the symbol set, and parameter layout. A retarget must update the pin and its size assertions in the same change.
