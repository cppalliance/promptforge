//! Section walk control flow over the offline fixtures: `call`, `jump`,
//! `fanout`, the `prompt` reads, and `var`. Each submodule holds one topic
//! and builds its prompts with the parser's source builder and runs them as
//! a [`Scenario`].

use runvil_parser::test_support::{PromptSource, one_section, prompt};

use crate::test_support::scenario::Scenario;

mod chains;
mod child_walks;
mod fanout_arms;
mod fanout_collections;
mod globals;
mod h1_control;
mod prompt_reads;
mod run_setup;
mod store_failures;
