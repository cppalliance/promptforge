//! Runvil's `require` in whole prompts: what a library hands back,
//! how often it loads, what it reaches, what its prose renders, what the
//! walk does when it reaches a library, and how library failures raise
//! and report. The cases run as a [`Scenario`], and the cases whose
//! libraries make model and tool calls run on the scripted model and a
//! fixture tool. Each submodule holds one topic and reaches the support
//! below through `super`.

use runvil_parser::test_support::{PromptSource, prompt};

use crate::test_support::scenario::{Record, Scenario};

/// The run name every `require` case runs under.
const REQUIRE_EXECUTION: &str = "fixture-require";

/// A `require` case over `source`.
fn require_case(source: String) -> Scenario {
    Scenario::new(source).name(REQUIRE_EXECUTION)
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
