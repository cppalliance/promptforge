//! The coroutine shim protocol tests: the yield shims installed on a
//! scheduler-mode section VM produce well-formed protocol requests.
//!
//! These live in `promptforge-core` (not in `promptforge-lua`) because the
//! real setup path they exercise is the executor's `section_vm` composition,
//! which stays with the executor to keep the dependency one-directional.

use std::num::NonZeroU32;
use std::sync::Arc;

use mlua::{MultiValue, Thread, Value};
use serde_json::json;

use promptforge_lua::Error;

use crate::execute::protocol::Request;
use crate::execute::section_vm::{SectionVmSetup, VmSeed, setup_section_vm};
use crate::lua::{CoroStep, LuaBlockResult, LuaProgram, SectionVm, ToolSet};
use crate::model::{ModelBinding, ModelId, ModelInvocation, ModelSet};
use crate::observe::{NullObserver, Observer};
use crate::store::StoreRef;
use crate::untrusted::GuardNonce;

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

/// Builds a section VM through the real setup path: construction, host
/// injection, the control surface with the yield shims, the shared
/// replay, and the captured alias bindings.
fn scheduler_vm(models: &ModelSet, var: Option<&serde_json::Value>) -> SectionVm {
    let observer: Arc<dyn Observer> = Arc::new(NullObserver::default());
    let mut vm = SectionVm::new_for_section(
        &GuardNonce::fresh(),
        &ToolSet::default(),
        models,
        "test-run",
        &NullObserver::default(),
        "Test",
    )
    .expect("the section VM builds");
    let shared = LuaProgram::empty().expect("the empty shared program compiles");
    let sys = json!({});
    let store = StoreRef::memory();
    let setup = SectionVmSetup {
        args: "",
        sys: &sys,
        store: &store,
        last_reply: None,
        seed: VmSeed { var, item: None },
        write_scope: None,
        observer_arc: &observer,
        section_name: "Test",
        shared: &shared,
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
fn execute_yields_target_input_and_the_var_snapshot() {
    let var = json!({ "k": 1 });
    let vm = scheduler_vm(&ModelSet::default(), Some(&var));
    match yielded_request(&vm, r###"return execute("## Child", "override")"###) {
        Request::Execute { target, input, var } => {
            assert_eq!(target, "## Child");
            assert_eq!(input.as_deref(), Some("override"));
            assert_eq!(var, json!({ "k": 1 }));
        }
        other => panic!("expected an execute request, got {other:?}"),
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
fn handle_infer_yields_the_inner_handle() {
    let vm = scheduler_vm(&test_models(), None);
    let request = yielded_request(
        &vm,
        r#"
            local h = models.get("fast")
            local u = models.use("fast")
            assert(h.name == "fast" and h.model_id == "test-model")
            assert(u.name == "fast")
            return h:infer("yo")
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
fn captured_model_aliases_install_as_shimmed_proxies() {
    let vm = scheduler_vm(&test_models(), None);
    match yielded_request(&vm, r#"return fast:infer("yo")"#) {
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
fn a_shimmed_handle_hides_its_inner_userdata() {
    let vm = scheduler_vm(&test_models(), None);
    // `getmetatable` survives hardening; the sealed proxy metatable is
    // the only thing keeping the inner userdata (and its non-yielding
    // Rust `infer` method) out of author reach.
    let (_thread, returned) = start(
        &vm,
        "return getmetatable(fast), getmetatable(models.get(\"fast\"))",
    );
    let values: Vec<Value> = returned.into_iter().collect();
    assert_eq!(values, vec![Value::Boolean(false), Value::Boolean(false)]);
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
        "var = 5\nexecute(\"## Child\")",
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

#[test]
fn the_budget_hook_fires_inside_a_resumed_coroutine() {
    // Spike (a): instruction hooks are per-coroutine in PUC Lua, so the
    // main-state hook installed at construction cannot bite here. The
    // block coroutine carries the VM's hook via `Thread::set_hook`; if
    // that install regressed, this loop would hang the test instead of
    // erroring.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let program = compile_block("while true do end");
    match vm.start_block_coro(&program) {
        Err(error) => assert!(
            matches!(
                error,
                Error::LuaQuota {
                    resource: "instruction"
                }
            ),
            "the per-coroutine hook must exhaust the instruction budget: {error:?}"
        ),
        other => panic!("an infinite loop can only fail, got {other:?}"),
    }
}

#[test]
fn the_instruction_budget_spans_block_coroutines_on_one_vm() {
    // One counter covers every chunk the VM runs: a block that exhausts
    // the budget leaves none for the next block's coroutine, so the
    // second block's first hook firing already trips the quota. A
    // per-thread fresh counter would let the second block finish.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let first = compile_block("while true do end");
    assert!(
        matches!(
            vm.start_block_coro(&first),
            Err(Error::LuaQuota {
                resource: "instruction"
            })
        ),
        "block one must exhaust the shared budget"
    );
    let second = compile_block("for i = 1, 100000 do end\nreturn \"done\"");
    match vm.start_block_coro(&second) {
        Err(error) => assert!(
            matches!(
                error,
                Error::LuaQuota {
                    resource: "instruction"
                }
            ),
            "block two inherits the exhausted budget: {error:?}"
        ),
        other => panic!("a fresh per-block budget would let block two finish: {other:?}"),
    }
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
