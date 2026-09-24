//! Tests for the scheduler-mode section VM built through the executor's
//! real `section_vm` setup path: the yield shims (`shims`), the error
//! table and failure envelope contract (`errors`), the coroutine
//! mechanics the shims rely on (`coroutine`), and the Lua loop's
//! instruction cost (`quota`). This file holds the fixtures every sibling
//! drives: the test model and tool sets, the VM builder, and the
//! start-and-parse helpers.
//!
//! These sit in `promptforge-engine` (not in `promptforge-lua`)
//! because the real setup path they exercise is the executor's
//! `section_vm` composition, which stays with the executor to keep the
//! dependency one-directional.

mod coroutine;
mod errors;
mod quota;
mod shims;

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use mlua::{MultiValue, Thread};
use serde_json::json;

use crate::execute::protocol::{Request, YieldParse};
use crate::execute::section_vm::{SectionVmSetup, VmSeed, setup_section_vm};
use crate::lua::{LuaProgram, SectionVm, ToolBinding, ToolSet};
use crate::model::{ModelBinding, ModelSet};
use crate::test_support::recording::null_emitter;
use crate::tools::ToolId;
use crate::untrusted::GuardNonce;
use promptforge_model_client::model::ModelInvocation;
use promptforge_types::detail::model_id_from_validated;
use promptforge_types::tools::ToolDescriptor;

fn test_models() -> ModelSet {
    ModelSet {
        bindings: vec![ModelBinding::new(
            "fast",
            "a fast model",
            model_id_from_validated("gateway", "test-model"),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking: None,
            },
            NonZeroU32::new(4096).expect("4096 is non-zero"),
        )],
        default: None,
    }
}

/// A minimal tool descriptor behind a bound alias, for handle-form
/// dispatch tests; dispatch never reaches an implementation through the
/// yield boundary, so the tool is data alone.
fn stub_tool() -> ToolDescriptor {
    ToolDescriptor::new(
        ToolId::parse("tests/tools/echo").expect("valid id"),
        "echo",
        "echo tool",
        json!({ "type": "object" }),
    )
}

/// One frozen tool set with the `echo` alias bound to the stub tool.
fn test_tools() -> ToolSet {
    ToolSet::for_test(
        vec![ToolBinding::for_test("echo", "echo tool", &stub_tool())],
        Vec::new(),
    )
}

/// Builds a section VM through the real setup path: construction, host
/// injection, the control surface with the yield shims, the shared
/// replay, and the captured alias bindings.
fn scheduler_vm(models: &ModelSet, var: Option<&serde_json::Value>) -> SectionVm {
    scheduler_vm_with_tools(models, &ToolSet::default(), var)
}

/// [`scheduler_vm`] with an explicit frozen tool set, so the captured
/// tool alias globals install as inspectable Tool objects.
fn scheduler_vm_with_tools(
    models: &ModelSet,
    tools: &ToolSet,
    var: Option<&serde_json::Value>,
) -> SectionVm {
    let emitter = null_emitter();
    let mut vm = SectionVm::new_for_section(
        &GuardNonce::from_seed(0x7e57),
        &Arc::new(Mutex::new(tools.clone())),
        &Arc::new(Mutex::new(models.clone())),
        &emitter,
        "Test",
    )
    .expect("the section VM builds");
    let shared = LuaProgram::empty().expect("the empty shared program compiles");
    let sys = json!({});
    let access = Arc::new(
        promptforge_vfs::empty()
            .acquire(promptforge_vfs::Origin::new("coroutine test fixture"))
            .expect("the stock backend acquires"),
    );
    let setup = SectionVmSetup {
        args: "",
        argv: None,
        argv_writable: false,
        sys: &sys,
        access: &access,
        seed: VmSeed { var, item: None },
        emitter: &emitter,
        section_name: "Test",
        shared: &shared,
        max_tool_iterations: 24,
        max_fanout_concurrency: 8,
        ui: None,
        raw_shims: false,
    };
    let list_callback =
        |_: String| -> std::result::Result<Vec<String>, crate::Error> { Ok(Vec::new()) };
    setup_section_vm(&mut vm, &setup, list_callback).expect("the setup installs");
    vm
}

/// Starts `source` as a coroutine on the VM and runs it to its first
/// yield, returning the thread and the yielded values.
fn start(vm: &SectionVm, source: &str) -> (Thread, MultiValue) {
    let function = vm
        .lua()
        .load(source)
        .into_function()
        .expect("the driver chunk compiles");
    let thread = vm
        .lua()
        .create_thread(function)
        .expect("the driver thread creates");
    let yielded = thread
        .resume::<MultiValue>(())
        .expect("the driver yields its request");
    (thread, yielded)
}

fn yielded_request(vm: &SectionVm, source: &str) -> Request {
    let (_thread, yielded) = start(vm, source);
    let value = yielded.into_iter().next().expect("one yielded value");
    match Request::from_yield(vm.lua(), &value) {
        YieldParse::Request(request) => request,
        other => panic!("the shim yield is a well-formed request, got {other:?}"),
    }
}

/// Compiles one author block the way the parser's prologue chunks are
/// compiled.
fn compile_block(source: &str) -> LuaProgram {
    LuaProgram::compile(
        source,
        "section `Test` prologue",
        NonZeroU32::MIN,
        &null_emitter(),
        "Test",
    )
    .expect("the driver block compiles")
}
