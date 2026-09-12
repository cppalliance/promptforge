//! The coroutine shim protocol tests: the yield shims installed on a
//! scheduler-mode section VM produce well-formed protocol requests.
//!
//! These live in `promptforge-core` (not in `promptforge-lua`) because the
//! real setup path they exercise is the executor's `section_vm` composition,
//! which stays with the executor to keep the dependency one-directional.

use std::num::NonZeroU32;
use std::sync::Arc;

use mlua::{MultiValue, Thread};
use serde_json::json;

use promptforge_lua::Error;

use crate::cancel::CancelHandle;
use crate::execute::protocol::Request;
use crate::execute::section_vm::{SectionVmSetup, VmSeed, setup_section_vm};
use crate::lua::{CoroStep, LuaBlockResult, LuaProgram, SectionVm, ToolBinding, ToolSet};
use crate::model::{ModelBinding, ModelId, ModelSet};
use crate::observe::{NullObserver, Observer};
use crate::tools::{Tool, ToolError, ToolId, ToolOutput};
use crate::untrusted::GuardNonce;
use promptforge_core_support::cancel::scope;
use promptforge_model_client::model::ModelInvocation;

fn test_models() -> ModelSet {
    ModelSet {
        bindings: vec![ModelBinding::new(
            "fast",
            "a fast model",
            ModelId::from_validated("gateway", "test-model"),
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

/// A minimal live tool behind a bound alias, for handle-form dispatch
/// tests; dispatch never reaches its `call` through the yield boundary.
struct StubTool;

#[async_trait::async_trait]
impl Tool for StubTool {
    fn id(&self) -> ToolId {
        ToolId::new("tests", "echo").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn wire_name(&self) -> &str {
        "echo"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "echo tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    async fn call(&self, _args: serde_json::Value) -> std::result::Result<ToolOutput, ToolError> {
        Ok(ToolOutput::trusted("echoed"))
    }
}

/// One frozen tool set with the `echo` alias bound to the stub tool.
fn test_tools() -> ToolSet {
    ToolSet::for_test(
        vec![ToolBinding::for_test(
            "echo",
            "echo tool",
            Arc::new(StubTool),
        )],
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
    let observer: Arc<dyn Observer> = Arc::new(NullObserver::default());
    let mut vm = SectionVm::new_for_section(
        &GuardNonce::fresh(),
        tools,
        models,
        "test-run",
        &NullObserver::default(),
        "Test",
    )
    .expect("the section VM builds");
    let shared = LuaProgram::empty().expect("the empty shared program compiles");
    let sys = json!({});
    let access = Arc::new(
        promptforge_vfs::empty()
            .acquire(shared_vfs::Origin::new("coroutine test fixture"))
            .expect("the stock backend acquires"),
    );
    let setup = SectionVmSetup {
        args: "",
        sys: &sys,
        access: &access,
        seed: VmSeed { var, item: None },
        observer_arc: &observer,
        section_name: "Test",
        shared: &shared,
        ui: None,
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
        crate::execute::protocol::YieldParse::Request(request) => request,
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
        "test-run",
        &NullObserver::default(),
        "Test",
    )
    .expect("the driver block compiles")
}

#[test]
fn models_infer_yields_a_well_formed_request() {
    let vm = scheduler_vm(&ModelSet::default(), None);
    match yielded_request(&vm, r#"return models.infer("summarize this")"#) {
        Request::Infer { prompt, binding } => {
            assert_eq!(prompt, "summarize this");
            assert_eq!(binding, None);
        }
        other => panic!("expected an infer request, got {other:?}"),
    }
}

#[test]
fn call_yields_target_input_and_the_var_snapshot() {
    let var = json!({ "k": 1 });
    let vm = scheduler_vm(&ModelSet::default(), Some(&var));
    match yielded_request(&vm, r###"return call("## Child", "override")"###) {
        Request::Call { target, input, var } => {
            assert_eq!(target, "## Child");
            assert_eq!(input.as_deref(), Some("override"));
            assert_eq!(var, json!({ "k": 1 }));
        }
        other => panic!("expected a call request, got {other:?}"),
    }
}

#[test]
fn fanout_yields_a_well_formed_request() {
    // The fanout shim is installed in scheduler mode: the global exists
    // and its yield parses into the protocol's Fanout variant, with the
    // collection converted member-wise at the boundary.
    let vm = scheduler_vm(&ModelSet::default(), None);
    match yielded_request(&vm, r####"return fanout("### Worker", {"a", "b"})"####) {
        Request::Fanout { worker, items, var } => {
            assert_eq!(worker, "### Worker");
            assert_eq!(items, vec![json!("a"), json!("b")]);
            assert_eq!(var, json!({}));
        }
        other => panic!("expected a fanout request, got {other:?}"),
    }
}

#[test]
fn tools_call_yields_a_well_formed_request() {
    // The tools.call shim installs in section VMs through the same setup
    // path as the other suspending calls; its yield parses into the
    // protocol's ToolCall variant with the author's args as JSON.
    let vm = scheduler_vm(&ModelSet::default(), None);
    match yielded_request(&vm, r#"return tools.call("echo", { value = "hi" })"#) {
        Request::ToolCall { alias, args } => {
            assert_eq!(alias, "echo");
            assert_eq!(args, json!({ "value": "hi" }));
        }
        other => panic!("expected a tool_call request, got {other:?}"),
    }
}

#[test]
fn the_bare_tool_call_global_is_not_installed() {
    // Every tool operation lives under the `tools.*` namespace; the bare
    // global from before the rename must be gone, not aliased.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let is_nil: bool = vm
        .lua()
        .load("return tool_call == nil")
        .eval()
        .expect("the global read evaluates");
    assert!(is_nil, "the bare `tool_call` global must not exist");
}

#[test]
fn tools_call_accepts_a_tool_handle_in_place_of_the_alias() {
    // The captured alias global is an inspectable Tool object; passing it
    // as the leading argument dispatches the binding it names.
    let vm = scheduler_vm_with_tools(&ModelSet::default(), &test_tools(), None);
    match yielded_request(&vm, r#"return tools.call(echo, { value = "hi" })"#) {
        Request::ToolCall { alias, args } => {
            assert_eq!(alias, "echo");
            assert_eq!(args, json!({ "value": "hi" }));
        }
        other => panic!("expected a tool_call request, got {other:?}"),
    }
}

#[test]
fn tools_call_rejects_a_non_alias_non_tool_first_argument() {
    // The polymorphism is alias string or Tool object; anything else is
    // the call's own error at the protocol boundary, so an author pcall
    // catches it at the call site.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let (_thread, yielded) = start(&vm, "return tools.call(42, {})");
    let value = yielded.into_iter().next().expect("one yielded value");
    match Request::from_yield(vm.lua(), &value) {
        crate::execute::protocol::YieldParse::Call(answer) => {
            let message = format!("{answer:?}");
            assert!(
                message.contains("tools.call alias must be a string or Tool object"),
                "the rejection names the expected forms: {message}"
            );
        }
        other => panic!("expected the call's own error, got {other:?}"),
    }
}

#[test]
fn models_infer_takes_an_optional_leading_handle() {
    let vm = scheduler_vm(&test_models(), None);
    let request = yielded_request(
        &vm,
        r#"
            local h = models.get("fast")
            local u = models.use("fast")
            assert(h.name == "fast" and h.model_id == "test-model")
            assert(u.name == "fast")
            return models.infer(h, "yo")
            "#,
    );
    match request {
        Request::Infer {
            prompt,
            binding: Some(binding),
        } => {
            assert_eq!(prompt, "yo");
            assert_eq!(binding.alias(), "fast");
            assert_eq!(binding.id().name(), "test-model");
        }
        other => panic!("expected an infer request with a binding, got {other:?}"),
    }
}

#[test]
fn captured_model_aliases_install_as_plain_handles() {
    let vm = scheduler_vm(&test_models(), None);
    match yielded_request(&vm, r#"return models.infer(fast, "yo")"#) {
        Request::Infer {
            prompt,
            binding: Some(binding),
        } => {
            assert_eq!(prompt, "yo");
            assert_eq!(binding.alias(), "fast");
        }
        other => panic!("expected an infer request with a binding, got {other:?}"),
    }
}

#[test]
fn handles_carry_no_colon_methods() {
    // Namespace-only invocation: a handle is a frozen, inspectable value,
    // so the old `handle:infer` method is gone - reading `infer` off the
    // userdata fails, and the one invocation form is the leading handle
    // argument to `models.infer`.
    let vm = scheduler_vm(&test_models(), None);
    let (is_userdata, read_failed): (bool, bool) = vm
        .lua()
        .load(
            r#"
            local h = models.get("fast")
            local ok = pcall(function() return h.infer end)
            return type(h) == "userdata" and type(fast) == "userdata", not ok
            "#,
        )
        .eval()
        .expect("the handle probe evaluates");
    assert!(is_userdata, "handles install as bare userdata");
    assert!(read_failed, "a handle has no `infer` field to call");
}

#[test]
fn models_infer_rejects_a_third_argument() {
    // `models.infer(handle?, prompt)` is the whole signature; a third
    // argument (per-call options, or anything else) raises at the call
    // site rather than being silently dropped.
    let vm = scheduler_vm(&test_models(), None);
    let program = compile_block(
        r#"local ok, err = pcall(models.infer, models.get("fast"), "yo", { temperature = 0 })
           assert(not ok, "a third argument must fail")
           assert(err == "models.infer takes (handle?, prompt)", err)
           return "rejected""#,
    );
    match vm.start_block_coro(&program).expect("the block runs") {
        CoroStep::Done(LuaBlockResult::Returned(Some(text))) => assert_eq!(text, "rejected"),
        other => panic!("expected the rejection return, got {other:?}"),
    }
}

#[test]
fn an_error_envelope_raises_at_the_call_site_without_a_position_prefix() {
    let vm = scheduler_vm(&ModelSet::default(), None);
    let (thread, _yielded) = start(&vm, r#"return models.infer("hi")"#);
    let error = thread
        .resume::<MultiValue>((false, "model is down"))
        .expect_err("the shim raises the envelope's message");
    // The raised error's message line is exactly the envelope string:
    // `error(result, 0)` suppresses the position prefix. (mlua appends
    // the traceback to the payload; that is its own rendering, not a
    // prefix on the message.)
    let mlua::Error::RuntimeError(message) = &error else {
        panic!("expected a runtime error, got {error:?}");
    };
    let first_line = message.lines().next().expect("a message line");
    assert_eq!(first_line, "model is down");
}

#[test]
fn a_traceback_through_a_shim_shows_unmapped_impl_frames() {
    let vm = scheduler_vm(&ModelSet::default(), None);
    // The var_snapshot capture fails on a reassigned `var` global: an
    // unexpected shim error, whose frames must render verbatim.
    let program = LuaProgram::compile(
        "var = 5\ncall(\"## Child\")",
        "section `Test` prologue",
        NonZeroU32::new(40).expect("40 is non-zero"),
        "test-run",
        &NullObserver::default(),
        "Test",
    )
    .expect("the driver program compiles");
    let function = program.load(vm.lua()).expect("the driver program loads");
    let thread = vm
        .lua()
        .create_thread(function)
        .expect("the driver thread creates");
    let error = thread
        .resume::<MultiValue>(())
        .expect_err("the reassigned var fails the snapshot");
    let raw = error.to_string();
    assert!(
        raw.contains("crates/promptforge-core/src/lua/__impl_coro.lua:"),
        "the shim frame renders as a verbatim file:line: {raw}"
    );
    assert!(
        !raw.contains("[string \"@crates") && !raw.contains("[string \"crates"),
        "the shim frame carries no [string \"...\"] wrapper: {raw}"
    );
    assert!(
        raw.contains("[string \"section `Test` prologue\"]:2:"),
        "the author frame is present at chunk line 2: {raw}"
    );
    let mapped = program.map_runtime_error(&error).to_string();
    assert!(
        mapped.contains("crates/promptforge-core/src/lua/__impl_coro.lua:"),
        "the line mapper leaves the shim frame unmapped: {mapped}"
    );
    assert!(
        mapped.contains("[string \"section `Test` prologue\"]:41:"),
        "the author frame maps to the absolute prompt line: {mapped}"
    );
}

#[tokio::test]
async fn the_cancellation_hook_fires_inside_a_resumed_coroutine() {
    // Spike (a): instruction hooks are per-coroutine in PUC Lua, so the
    // main-state hook installed at construction cannot bite here. The
    // block coroutine carries the VM's hook via `Thread::set_hook`; no
    // instruction ceiling remains, so if that install regressed, this
    // pre-cancelled loop would hang the test instead of aborting.
    let handle = CancelHandle::new();
    handle.cancel();
    let outcome = scope(handle, async {
        let vm = scheduler_vm(&ModelSet::default(), None);
        let program = compile_block("while true do end");
        vm.start_block_coro(&program)
    })
    .await;
    match outcome {
        Err(error) => assert!(
            matches!(error, Error::Interrupted),
            "the per-coroutine hook must observe cancellation: {error:?}"
        ),
        other => panic!("a cancelled infinite loop can only fail, got {other:?}"),
    }
}

#[tokio::test]
async fn every_block_coroutine_carries_the_cancellation_hook() {
    // One VM installs the hook on every block coroutine it starts, not
    // only the first: under a cancelled run, each block's first hook
    // firing aborts it. A thread that missed the install would let the
    // second block hang (the loop) or finish (the bounded for), so either
    // block escaping cancellation fails this test.
    let handle = CancelHandle::new();
    handle.cancel();
    scope(handle, async {
        let vm = scheduler_vm(&ModelSet::default(), None);
        for source in [
            "while true do end",
            "for i = 1, 100000 do end\nreturn \"done\"",
        ] {
            let program = compile_block(source);
            match vm.start_block_coro(&program) {
                Err(error) => assert!(
                    matches!(error, Error::Interrupted),
                    "block {source:?} must abort on the cancelled run: {error:?}"
                ),
                other => panic!("a cancelled block can only fail, got {other:?}"),
            }
        }
    })
    .await;
}

#[test]
fn a_shim_yield_suspends_and_resumes_across_pcall() {
    // Spike (b): yield across pcall (5.4+ semantics, re-confirmed on
    // 5.5). If yield could not cross the pcall boundary, the resume
    // would fail with "attempt to yield across a pcall boundary".
    let vm = scheduler_vm(&ModelSet::default(), None);
    let program = compile_block(
        "local ok, result = pcall(function() return models.infer(\"hi\") end)\n\
             assert(ok, result)\n\
             return \"pcall:\" .. result",
    );
    let CoroStep::Yielded(thread, values) =
        vm.start_block_coro(&program).expect("the block suspends")
    else {
        panic!("the shim yield must suspend the pcall'd block");
    };
    let value = values.into_iter().next().expect("one yielded value");
    let request = match Request::from_yield(vm.lua(), &value) {
        crate::execute::protocol::YieldParse::Request(request) => request,
        other => panic!("the shim yield is a well-formed request, got {other:?}"),
    };
    assert!(matches!(request, Request::Infer { .. }));
    match vm
        .resume_block_coro(&program, &thread, (true, "answer"))
        .expect("the suspended pcall resumes")
    {
        CoroStep::Done(LuaBlockResult::Returned(Some(text))) => {
            assert_eq!(text, "pcall:answer");
        }
        other => panic!("expected the resumed return, got {other:?}"),
    }
}

#[test]
fn jump_transfers_through_thread_resume_unchanged() {
    // Spike (c): `jump` records the heading and raises its transfer
    // marker; through `Thread::resume` the slot still takes precedence
    // over the chunk's error, so the outcome matches the legacy path.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let program = compile_block("jump(\"## Target\")\nerror(\"unreachable\")");
    match vm
        .start_block_coro(&program)
        .expect("a jump is not a failure")
    {
        CoroStep::Done(LuaBlockResult::Jump(heading)) => assert_eq!(heading, "## Target"),
        other => panic!("expected the jump transfer, got {other:?}"),
    }
}

#[test]
fn at_named_chunk_errors_render_verbatim_through_resume() {
    // Spike (d): `set_name` passes an `@`-prefixed chunk name through to
    // lua_load untouched, so an error in a chunk resumed via `Thread`
    // renders as a verbatim file:line: reference with no wrapper.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let program = LuaProgram::compile_internal(
        "local x = nil\nreturn x.field",
        "@crates/promptforge-core/src/lua/__impl_probe.lua",
    )
    .expect("the probe compiles");
    let error = match vm.start_block_coro(&program) {
        Err(error) => error,
        other => panic!("the probe must fail, got {other:?}"),
    };
    let raw = error.to_string();
    assert!(
        raw.contains("crates/promptforge-core/src/lua/__impl_probe.lua:2:"),
        "the error renders as a verbatim file:line: {raw}"
    );
    assert!(
        !raw.contains("[string \"@"),
        "the chunk name carries no [string \"...\"] wrapper: {raw}"
    );
}

#[test]
fn scalar_return_and_vm_state_roll_forward_across_block_coroutines() {
    // Chunk-return semantics: a block's scalar return survives the
    // coroutine boundary, and the VM state (`var`, `reply`) written by
    // one block's coroutine is visible to the next block's coroutine.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let first = compile_block("var.count = 41\nreply = \"rolled\"\nreturn \"first-result\"");
    match vm.start_block_coro(&first).expect("block one runs") {
        CoroStep::Done(LuaBlockResult::Returned(Some(text))) => {
            assert_eq!(text, "first-result");
        }
        other => panic!("expected block one's scalar return, got {other:?}"),
    }
    let second = compile_block("assert(var.count == 41)\nassert(reply == \"rolled\")\nreturn 42");
    match vm.start_block_coro(&second).expect("block two runs") {
        CoroStep::Done(LuaBlockResult::Returned(Some(text))) => assert_eq!(text, "42"),
        other => panic!("expected block two's scalar return, got {other:?}"),
    }
}
