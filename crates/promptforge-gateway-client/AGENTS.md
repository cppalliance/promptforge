# promptforge-gateway-client

This crate is the gateway's model client: the OpenAI-shaped chat-completions
transport (`GatewayClient`), the wire types it exchanges, the model catalog and
prompt-local binding vocabulary, and the semantic `models.bind` resolver
adapter over the tool picker.

## Rules

- Gateway model client only; never a universal client. Protocol-specific wire
  types stay protocol-specific; a future MCP or other tool client is a
  separate crate.
- No parser, Lua, or executor dependencies. The crate never imports
  `promptforge-core` subsystems (parser, `mlua`, execute, store, observe);
  core adapts to this crate, never the reverse.
- The `#[doc(hidden)]` items and `pub` fields marked as cross-crate seams are
  how `promptforge-core` reaches previously `pub(crate)` internals; they are
  not host API and must not gain documented status without a design change.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
