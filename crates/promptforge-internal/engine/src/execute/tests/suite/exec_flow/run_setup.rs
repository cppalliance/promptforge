//! What a run starts with: its store handle, a default context, and the
//! tool and model bindings its frontmatter declares.

use std::sync::Arc;

use super::*;

use crate::RunErrorKind;
use crate::RunResult;
use crate::execute::tests::context::test_context;
use crate::execute::tests::fixtures::echo_tool;
use crate::test_support::scenario::EXECUTION;
use promptforge_vfs::{VfsError, VfsRef};

/// A hand-built `VfsRef` that declares no store fails the run up front
/// with [`RunErrorKind::Vfs`], saying what is missing: every run needs a
/// declared store. (`RunContext::prepare` never replaces the handle, so
/// the raw handle's declaration is what the run sees.)
#[test]
fn a_handle_without_a_declared_store_fails_the_run() {
    let source = one_section("store.write('overlay.txt', 'overlaid')");
    let vfs = VfsRef::new(promptforge_vfs::MemoryBackend::new());
    let trace = Scenario::new(source).vfs(vfs).unprepared().run();
    let error = trace.error();
    assert_eq!(
        error.kind(),
        RunErrorKind::Vfs,
        "the run fails as a store error: {error}"
    );
    assert_eq!(
        error.to_string(),
        "this run declares no store, so no store operation can run"
    );
}

/// A store backend that refuses every session.
struct RefusingStore;

impl promptforge_vfs::Vfs for RefusingStore {
    fn acquire(
        &mut self,
        _cx: &promptforge_vfs::AcquireContext,
    ) -> std::result::Result<Box<dyn promptforge_vfs::VfsAccess>, VfsError> {
        Err(VfsError::Backend {
            message: "the store backend refuses every session".to_owned(),
        })
    }

    fn release(&mut self, _id: promptforge_vfs::ExecId) -> std::result::Result<(), VfsError> {
        Ok(())
    }
}

#[test]
fn a_store_backend_that_refuses_its_session_fails_the_run_at_the_first_step() {
    let source = one_section("return 'never runs'");
    let (prompt, _) = crate::parser::Prompt::parse(&source, EXECUTION);
    let vfs = VfsRef::builder().store("/", RefusingStore).build();
    let mut run = crate::execute::run::Run::new(
        Arc::new(prompt.expect("the fixture parses")),
        "",
        test_context(EXECUTION).vfs(vfs),
    );
    match run.step() {
        crate::execute::run::Step::Done {
            result: RunResult::Failure(error),
            ..
        } => assert_eq!(error.kind(), RunErrorKind::Vfs, "{error}"),
        other => panic!("expected the first step to fail the run, got {other:?}"),
    }
}

#[test]
fn a_default_context_runs_a_plugin_free_prompt() {
    // A prompt declaring no Plugins runs under a default context: no
    // catalog, no roster, no client, no real roots.
    let trace = Scenario::new(one_section("return 'no Plugins'")).run();
    assert_eq!(trace.text(), "no Plugins");
}

#[test]
fn default_run_context_store_handle_declares_a_fresh_store() {
    // `RunContext` absorbs the filesystem handle with a `VfsRef::default()`
    // (a fresh memory store at `/`) default: a store-using run needs no
    // caller-supplied handle.
    let source = prompt(
        "",
        &[
            ("First", "store.write('default.txt', 'stock')"),
            ("Second", "return store.read('default.txt')"),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "stock");
}

#[test]
fn offering_a_tool_its_present_plugin_does_not_offer_fails_the_section() {
    // An id whose Plugin is active but contributed no such tool is no
    // catalog tool of the run: the offer fails the section before any
    // round advertises it. The catalog holds the Plugin's `echo`, never
    // `search`.
    let source = prompt(
        "plugins: [tools]",
        &[(
            "Only",
            "
            tools.offer('tools/search')
            return 'unreachable'",
        )],
    );
    let trace = Scenario::new(source).tool(echo_tool()).run();
    let error = trace.error();
    assert_eq!(error.kind(), RunErrorKind::Lua, "{error}");
    assert!(
        error
            .to_string()
            .contains("tools.offer: \"tools/search\" is not a catalog tool in this run"),
        "the failure names the id: {error}"
    );
}
