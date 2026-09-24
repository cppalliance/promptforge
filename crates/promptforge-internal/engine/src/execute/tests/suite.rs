//! Offline prompt-fixture suite: parses fixtures written the way a prompt
//! author writes them and drives their execution against the runtime
//! without a live gateway. Split by domain - parsing contracts, section
//! execution, fanout, control flow, the args/argv and lazy-prose surfaces,
//! and the prepare pass - over shared harness code in [`support`]. Its
//! cases reach engine-only items (the test drivers and recorders, the store
//! facade, the parser's Lua programs), so they run here rather than
//! against the `promptforge` facade, whose own suite holds the rest.

mod args_surface;
mod exec_flow;
mod execution;
mod fanout;
mod lazy_prose;
mod parsing;
mod prepare;
mod support;
mod vfs;
