//! Offline prompt-fixture suite: parses fixtures written the way a prompt
//! author writes them, through the public parser, and drives their execution
//! against the runtime without a live gateway. Split by domain - parsing
//! contracts, section execution, fanout, control flow, the args/argv and
//! lazy-prose surfaces, and shipped-prompt policy - over shared harness code
//! in [`support`].
#![expect(
    clippy::expect_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod args_surface;
mod exec_flow;
mod execution;
mod fanout;
mod lazy_prose;
mod parsing;
mod prepare;
mod shipped;
mod support;
mod vfs;
