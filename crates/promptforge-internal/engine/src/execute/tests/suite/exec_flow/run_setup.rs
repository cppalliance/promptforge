//! What a run starts with: its store handle, the default environment,
//! and the tool and model bindings its frontmatter declares.

use super::*;

use crate::RunErrorKind;
use crate::test_support::recording::Observer;
use crate::test_support::{RunHarness, TestToolTable, run_with_harness};
use crate::{Environment, RunResult};
use promptforge_types::tools::{ToolError, ToolId, ToolOutput};
use promptforge_vfs::VfsRef;
use serde_json::{Value, json};

use super::super::support::{Recorder, context, parse_execution_fixture};

/// A hand-built `VfsRef` that declares no store fails the run up front
/// with [`RunErrorKind::Vfs`]: every run needs a declared store, and
/// the defensive fallback overlay is gone. (`Environment::prepare` never
/// replaces the handle, so the raw handle's declaration is what the run
/// sees.)
#[tokio::test]
async fn a_handle_without_a_declared_store_fails_the_run() {
    let md = flow_prompt!(
        "# Test prompt\n\n\
        ## Only\n\n```lua\nstore.write('overlay.txt', 'overlaid')\n```\n"
    );
    let vfs = VfsRef::new(promptforge_vfs::MemoryBackend::new());
    let result = run_fixture(md, "exec-flow", EXECUTION, "", Some(vfs))
        .await
        .result
        .expect_err("a run whose handle declares no store must be refused");
    assert_eq!(
        result.kind(),
        crate::RunErrorKind::Vfs,
        "the run fails as a store error: {result}"
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
    let md = flow_prompt!("# Test prompt\n\n## Only\n\n```lua\nreturn 'never runs'\n```\n");
    let (prompt, _) = crate::parser::Prompt::parse(md, EXECUTION);
    let vfs = VfsRef::builder().store("/", RefusingStore).build();
    let mut run = crate::execute::run::Run::new(
        Arc::new(prompt.expect("the fixture parses")),
        "",
        context(EXECUTION).vfs(vfs),
    );
    match run.step() {
        crate::execute::run::Step::Done {
            result: RunResult::Failure(error),
            ..
        } => assert_eq!(error.kind(), RunErrorKind::Vfs, "{error}"),
        other => panic!("expected the first step to fail the run, got {other:?}"),
    }
}

#[tokio::test]
async fn default_environment_runs_a_plugin_free_prompt() {
    // A prompt with no capability binds runs under the default
    // `Environment`: no registry, no client, no real roots.
    let md = flow_prompt!(
        "# Test prompt\n\n\
        ## Only\n\n```lua\nreturn 'no Plugins'\n```\n"
    );
    let out = run_fixture(md, "exec-flow", EXECUTION, "", None)
        .await
        .result
        .expect("a Plugin-free prompt runs under the default environment");
    assert_eq!(out, "no Plugins");
}

#[tokio::test]
async fn default_run_context_store_handle_declares_a_fresh_store() {
    // `RunContext` absorbs the filesystem handle with a `VfsRef::default()`
    // (a fresh memory store at `/`) default: a store-using run needs no
    // Harness-supplied handle.
    let md = flow_prompt!(
        "# Test prompt\n\n\
        ## First\n\n```lua\nstore.write('default.txt', 'stock')\n```\n\n\
        ## Second\n\n```lua\nreturn store.read('default.txt')\n```\n"
    );
    let out = run_fixture(md, "exec-flow", EXECUTION, "", None)
        .await
        .result
        .expect("the default store handle declares a fresh store");
    assert_eq!(out, "stock");
}

#[tokio::test]
async fn advertising_an_unfilled_slot_fails_at_run_time() {
    // An exact slot whose capability is active but contributed no such tool
    // stays unfilled at prepare (the capability is not missing, so the run is
    // not refused); advertising the alias in a section is the run-time error
    // prepare promised.
    let md = concat!(
        "---\nname: t\ndescription: d\npromptforge: 0\nplugins:\n  - tests/tools\ntools:\n  search: tests/tools/search\n---\n\n",
        "# Test prompt\n\n\
        ## Only\n\n```lua\ntools.add('search')\nreturn 'unreachable'\n```\n"
    );
    let observer: Arc<dyn Observer> = Arc::new(Recorder::default());
    let prompt = parse_execution_fixture(md, "exec-flow", EXECUTION, observer.as_ref());
    // The catalog holds the capability's `echo`, never `search`: the slot's
    // capability is present, so the slot is unfilled, not missing.
    let tool: Arc<dyn TestTool> = Arc::new(EchoTool);
    let table = TestToolTable::from_tools(&[tool]);
    let catalog = table
        .catalog()
        .expect("the fixture tools have legal wire names and distinct ids");
    let RunResult::Failure(error) = run_with_harness(
        &Environment::new().tools(catalog),
        &prompt,
        "",
        context(EXECUTION),
        RunHarness::new().tools(table),
    )
    .await
    else {
        panic!("advertising an unfilled alias must fail");
    };
    assert!(
        error
            .to_string()
            .contains("tools.add alias \"search\" is not a bound tool slot"),
        "the failure names the unfilled alias: {error}"
    );
}

#[tokio::test]
async fn models_bind_is_gone_from_the_lua_surface() {
    // `models.bind` does not exist: binding is the frontmatter's. A
    // `models.bind` call is a nil call, and the failed H1 gate classifies as
    // RequirementsUnmet.
    let md = flow_prompt!(
        "# Test prompt\n\n\
        ```lua\nmodels.bind('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\n```lua\nreturn 'unreachable'\n```\n"
    );
    let observer: Arc<dyn Observer> = Arc::new(Recorder::default());
    let prompt = parse_execution_fixture(md, "exec-flow", EXECUTION, observer.as_ref());
    let env = Environment::new();
    let RunResult::Failure(error) =
        run_with_harness(&env, &prompt, "", context(EXECUTION), RunHarness::new()).await
    else {
        panic!("a models.bind call must fail");
    };
    assert_eq!(
        error.kind(),
        RunErrorKind::RequirementsUnmet,
        "the removed models.bind fails the H1 gate as RequirementsUnmet: {error:?}"
    );
}

/// The fixture capability's `echo` tool, present so a prompt's `search` slot
/// is unfilled rather than its capability missing. The implementation is never
/// called by the unfilled-slot case.
struct EchoTool;

#[async_trait::async_trait]
impl TestTool for EchoTool {
    fn id(&self) -> ToolId {
        ToolId::parse("tests/tools/echo").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "echo"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Echo the value argument back to the caller."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"]
        })
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let value = args
            .get("value")
            .and_then(Value::as_str)
            .expect("the fixture tool requires a string value");
        Ok(ToolOutput::trusted(format!("echoed: {value}")))
    }
}
