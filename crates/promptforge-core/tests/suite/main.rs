//! Offline prompt-fixture suite: parses author-shaped fixtures through the
//! public parser and drives their execution against the runtime without a live
//! gateway. Split by domain - parsing contracts, section execution, fanout, and
//! shipped-prompt policy - over shared harness code in [`support`].

mod execution;
mod fanout;
mod parsing;
mod shipped;
mod support;
