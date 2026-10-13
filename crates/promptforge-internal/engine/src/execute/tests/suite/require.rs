//! PromptForge's `require` in whole prompts: what a library hands back,
//! how often it loads, what it reaches, what its prose renders, what the
//! walk does when it reaches a library, and how library failures raise
//! and report. The cases run on the suite's `run_fixture` runner, and the
//! cases whose libraries make model and tool calls run on the scripted
//! gateway and the fixture tools. Each submodule holds one topic and
//! reaches the support below through `super`.

use crate::RunError;

use super::support::{Record, run_fixture};

const EXECUTION: &str = "fixture-require";

/// The frontmatter and H1 every `require` case shares, fused into the
/// prompt literal at compile time so a case states only its sections. The
/// first section heading sits on prompt line 9.
macro_rules! require_prompt {
    ($body:literal) => {
        concat!(
            "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n# Require\n\n",
            $body
        )
    };
}

/// Runs a `require` case offline with `args` and the run's own store.
async fn run_with_args(md: &str, args: &str) -> Result<String, RunError> {
    run_fixture(md, "require", EXECUTION, args, None)
        .await
        .result
}

/// Runs a `require` case offline with no args.
async fn run_offline(md: &str) -> Result<String, RunError> {
    run_with_args(md, "").await
}

/// The library-load reports among `records`, as `(section, detail)` pairs
/// in order.
fn library_loads(records: &[Record]) -> Vec<(String, String)> {
    records
        .iter()
        .filter(|record| record.detail.starts_with("Lua library load"))
        .map(|record| (record.section.clone(), record.detail.clone()))
        .collect()
}

/// A `(section, detail)` pair for comparing against [`library_loads`].
fn load(section: &str, detail: &str) -> (String, String) {
    (section.to_owned(), detail.to_owned())
}

mod effects;
mod failures;
mod loading;
mod prose;
mod reach;
mod walk;
