-- Live H1 shim wrap, applied to each H1 block's freshly installed live
-- models table. `infer` and `wrap_handle` are the shim prelude's
-- privileged captures, passed in so the chunk never reads a global;
-- `models` is the block's live table. The `bind`/`default` returns wrap
-- into proxies so a handle's `infer` yields instead of calling the
-- non-yielding Rust method; `execute`/`fanout` keep their H1 stubs, which
-- raise before anything can yield.
local infer, wrap_handle, models = ...

models.infer = infer
local raw_bind, raw_default = models.bind, models.default
models.bind = function(...) return wrap_handle(raw_bind(...)) end
models.default = function(...) return wrap_handle(raw_default(...)) end
