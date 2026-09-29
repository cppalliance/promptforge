//! Section walk control flow over the offline harness: `call`, `jump`,
//! `fanout`, `list_from_section`, and `var`. The in-crate cases reached the
//! private `run`/`fixture`/`TestStore`/`silent`/`run_offline` helpers through
//! `use super::*`; the moved cases keep their assertions and are rebuilt on
//! the suite's `run_fixture` runner through the thin offline harness below.
//! Each submodule holds one topic and reaches that harness through `super`.

use std::sync::{Arc, Mutex};

use crate::RunError;
use crate::test_support::TestTool;
use promptforge_vfs::VfsError;

use super::support::{FixtureRun, FixtureStore, run_fixture};

const EXECUTION: &str = "execute-test";

/// The parsed fixture a moved case runs: its source, parsed and driven by the
/// suite's `run_fixture` at call time.
struct TestPrompt<'a>(&'a str);

/// Builds a [`TestPrompt`] from an inline fixture source.
fn fixture(md: &str) -> TestPrompt<'_> {
    TestPrompt(md)
}

/// The execution id the moved cases run under.
struct RunOptions {
    execution: &'static str,
}

/// The silent options an offline flow case runs with.
fn silent() -> RunOptions {
    RunOptions {
        execution: EXECUTION,
    }
}

/// The store a moved case asserts on: the wrapper the in-crate `TestStore`
/// provided, filled from the run the local [`run`] drives.
struct TestStore(Mutex<Option<FixtureStore>>);

impl TestStore {
    fn new() -> TestStore {
        TestStore(Mutex::new(None))
    }

    fn read(&self, path: &str) -> Result<String, VfsError> {
        self.0
            .lock()
            .expect("the store lock is not poisoned")
            .as_ref()
            .expect("the local run installs the store")
            .read(path)
    }
}

/// Runs an offline flow fixture through the suite `run_fixture` runner: the
/// in-crate `run` helper's shape over the suite helpers, with the run's store
/// captured for post-run assertions.
async fn run(
    test: &TestPrompt<'_>,
    args: &str,
    _tools: &[Arc<dyn TestTool>],
    store: &TestStore,
    opts: RunOptions,
) -> Result<String, RunError> {
    let FixtureRun {
        result,
        store: run_store,
        ..
    } = run_fixture(test.0, "exec-flow", opts.execution, args, None).await;
    *store.0.lock().expect("the store lock is not poisoned") = Some(run_store);
    result
}

/// Runs an offline flow fixture with no args and no tools, returning the
/// result alone.
async fn run_offline(md: &str) -> Result<String, RunError> {
    run_fixture(md, "exec-flow", EXECUTION, "", None)
        .await
        .result
}

/// The frontmatter every flow test shares, fused into the prompt literal at
/// compile time so a test states only its sections.
macro_rules! flow_prompt {
    ($body:literal) => {
        concat!(
            "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n",
            $body
        )
    };
}

mod chains;
mod child_walks;
mod fanout_arms;
mod fanout_collections;
mod globals;
mod h1_control;
mod list_from_section;
mod run_setup;
mod store_failures;
