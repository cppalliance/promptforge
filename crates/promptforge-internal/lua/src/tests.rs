//! Crate-wide tests for section VMs: sandboxing, logging, tool scoping, store operations, `var`, and `argv`.
//!
//! Each submodule holds one topic. The helpers below are shared by them;
//! `recording`, `assert_chunk_name_resolves`, and `refusal_line` serve the
//! crate's other test modules too.

use std::sync::{Arc, Mutex};

use super::*;
use crate::vm::{LuaOutcome, run_chunk};
use promptforge_types::tools::ToolDescriptor;
use promptforge_vfs::{AcquireContext, ExecId, Origin, Vfs, VfsAccess, VfsError, VfsPath, VfsRef};
use serde_json::json;

#[path = "tests-recording.rs"]
pub(crate) mod recording;

use recording::{Observation, Recorder, detail, null_emitter};

mod argv;
mod budgets;
mod cancellation;
mod diagnostics;
mod logging;
mod sandbox;
mod section_vm;
mod shared_replay;
mod store;
mod store_errors;
mod store_reports;
mod sys_and_var;
mod tool_scoping;

/// A fresh default handle's access capability for a test VM: the store is
/// declared at the root and the vended identity is the test's own, so
/// seeding through the store view and the VM's store ops never meet a
/// second live identity.
fn fresh_access() -> Arc<Access> {
    Arc::new(
        VfsRef::default()
            .acquire(Origin::new("lua test fixture"))
            .expect("the stock backend acquires"),
    )
}

/// The store view derived from a test's access: the same view the `store`
/// table's closures operate through, for seeding and extraction.
fn store_view(access: &Access) -> Access {
    promptforge_vfs::detail::store_view(access).expect("the handle declares a store")
}

/// Returns the message held by either Lua-category error representation.
fn lua_error_message(error: &Error) -> &str {
    match error {
        Error::Lua(message) | Error::LuaRuntime { message, .. } => message,
        other => panic!("expected a Lua-category error, got {other:?}"),
    }
}

/// A backend whose every operation fails. The error is `Backend` rather
/// than `NotFound` so the facade's idempotent-delete mapping (absent is
/// `Ok`) cannot swallow the failure: every op must reach Lua as an error.
#[derive(Debug)]
struct FailingBackend;

impl FailingBackend {
    fn error(path: &VfsPath) -> VfsError {
        VfsError::Backend {
            message: format!("the failing backend rejects every operation: {path}"),
        }
    }
}

impl Vfs for FailingBackend {
    fn acquire(
        &mut self,
        cx: &AcquireContext,
    ) -> std::result::Result<Box<dyn VfsAccess>, VfsError> {
        let _ = cx;
        Ok(Box::new(FailingAccess))
    }

    fn release(&mut self, id: ExecId) -> std::result::Result<(), VfsError> {
        let _ = id;
        Ok(())
    }
}

struct FailingAccess;

impl VfsAccess for FailingAccess {
    fn read(&self, path: &VfsPath) -> std::result::Result<Vec<u8>, VfsError> {
        Err(FailingBackend::error(path))
    }

    fn write(&mut self, path: &VfsPath, _contents: &[u8]) -> std::result::Result<(), VfsError> {
        Err(FailingBackend::error(path))
    }

    fn append(&mut self, path: &VfsPath, _contents: &[u8]) -> std::result::Result<(), VfsError> {
        Err(FailingBackend::error(path))
    }

    fn remove(&mut self, path: &VfsPath, _recursive: bool) -> std::result::Result<(), VfsError> {
        Err(FailingBackend::error(path))
    }

    fn exists(&self, path: &VfsPath) -> std::result::Result<bool, VfsError> {
        Err(FailingBackend::error(path))
    }

    fn glob(&self, pattern: &str) -> std::result::Result<Vec<String>, VfsError> {
        Err(VfsError::Backend {
            message: format!("the failing backend rejects every operation: {pattern}"),
        })
    }

    fn list(&self, path: &VfsPath) -> std::result::Result<Vec<promptforge_vfs::Entry>, VfsError> {
        Err(FailingBackend::error(path))
    }

    fn stat(&self, path: &VfsPath) -> std::result::Result<promptforge_vfs::Stat, VfsError> {
        Err(FailingBackend::error(path))
    }

    fn mkdir(&mut self, path: &VfsPath, _recursive: bool) -> std::result::Result<(), VfsError> {
        Err(FailingBackend::error(path))
    }

    fn rename(&mut self, from: &VfsPath, _to: &VfsPath) -> std::result::Result<(), VfsError> {
        Err(FailingBackend::error(from))
    }

    fn copy(&mut self, from: &VfsPath, _to: &VfsPath) -> std::result::Result<(), VfsError> {
        Err(FailingBackend::error(from))
    }
}

/// The access a failing backend vends, for tests driving the error path:
/// the failing backend declared as the store, so the store view derives
/// and every operation reaches the backend's refusal.
fn failing_access() -> Arc<Access> {
    Arc::new(
        VfsRef::builder()
            .store("/", FailingBackend)
            .build()
            .acquire(Origin::new("failing backend test"))
            .expect("the failing backend still acquires"),
    )
}

fn run(source: &str, args: &str) -> Result<LuaOutcome> {
    run_chunk(
        source,
        args,
        &json!({ "id": 1, "when": "t" }),
        &fresh_access(),
        &null_emitter(),
        "Test",
    )
}

/// Mints the guard nonce for a test-owned VM; every wrap that VM's
/// `untrusted` global performs shares it, matching the per-run nonce the
/// executor mints.
fn test_nonce() -> GuardNonce {
    GuardNonce::from_seed(0x6c75_6174_6573)
}

/// Runs a chunk against a caller-supplied access, so a test can inspect the
/// store through the same identity after the chunk has run.
fn run_with(source: &str, access: &Arc<Access>) -> Result<LuaOutcome> {
    run_chunk(
        source,
        "",
        &json!({ "id": 1, "when": "t" }),
        access,
        &null_emitter(),
        "Test",
    )
}

/// Runs one chunk on an existing VM and unwraps the scalar return, failing
/// the test on a `jump` transfer.
fn run_scalar(
    vm: &SectionVm,
    program: &LuaProgram,
    emitter: &Emitter,
    section: &str,
) -> Result<Option<String>> {
    match vm.run_chunk(program, emitter, section)? {
        LuaBlockResult::Returned(value) => Ok(value),
        LuaBlockResult::Jump(heading) => Err(Error::Lua(format!("unexpected jump to {heading}"))),
    }
}

fn program(source: &str) -> LuaProgram {
    LuaProgram::compile(
        source,
        "test program",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        &null_emitter(),
        "Test",
    )
    .expect("test Lua must compile")
}

/// A fixture tool as data: `fixtures/tools/<name>`, advertised under its
/// name. The VM binds descriptors and yields calls; no implementation is
/// ever reached here.
fn fixture_tool(name: &str) -> ToolDescriptor {
    ToolDescriptor::new(
        ToolId::parse(&format!("fixtures/tools/{name}")).expect("valid id"),
        "fixture",
        json!({}),
    )
}

/// Builds a fixture tool set directly: each `(alias, description, fixture)`
/// triple is a bound slot, with `always` aliases parked prompt-wide. This is
/// the shape prepare's filled slots arrive in; no Lua runs to produce it.
fn fixture_set(bindings: &[(&str, &str, &'static str)], always: &[&str]) -> ToolSet {
    ToolSet::for_test(
        bindings
            .iter()
            .map(|(alias, description, fixture)| {
                ToolBinding::for_test(alias, description, &fixture_tool(fixture))
            })
            .collect(),
        always.iter().map(|alias| (*alias).to_owned()).collect(),
        Vec::new(),
    )
}

/// Shares a fixture set the way the run shares its own: one allocation every
/// section VM clones.
fn shared_set(bindings: ToolSet) -> Arc<Mutex<ToolSet>> {
    Arc::new(Mutex::new(bindings))
}

fn section_vm_with_set(
    tools: &Arc<Mutex<ToolSet>>,
    emitter: &Emitter,
    section: &str,
) -> Result<SectionVm> {
    let vm = SectionVm::new_for_section(
        &test_nonce(),
        tools,
        &Arc::new(Mutex::new(ModelSet::default())),
        emitter,
        section,
    )?;
    vm.install_captured_bindings()?;
    Ok(vm)
}

fn section_vm_with_bindings(
    bindings: &ToolSet,
    emitter: &Emitter,
    section: &str,
) -> Result<SectionVm> {
    section_vm_with_set(&shared_set(bindings.clone()), emitter, section)
}

/// Builds a section VM through the Engine's startup order for a shared
/// library: construction, Engine injection, persistent Engine globals, then
/// the shared replay. Tests that need control globals or captured bindings
/// add them by hand.
fn section_vm_with_shared(
    shared: &LuaProgram,
    args: &str,
    access: &Arc<Access>,
    emitter: &Emitter,
    section: &str,
) -> Result<SectionVm> {
    let mut vm = SectionVm::new(&test_nonce(), emitter, section)?;
    vm.inject_values(args, &json!({}), access)?;
    vm.install_engine_globals(emitter, section)?;
    vm.replay_shared(shared, emitter, section)?;
    Ok(vm)
}

/// A section VM with the coroutine shims installed, so the shim's `pcall`
/// and `xpcall` replacements are live, under `cancel` when one is given.
fn shim_vm(cancel: Option<promptforge_types::cancel::CancelHandle>) -> SectionVm {
    let emitter = null_emitter();
    let mut vm = SectionVm::new(&test_nonce(), &emitter, "Loop").expect("VM must build");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("Engine values must inject");
    vm.install_engine_globals(&emitter, "Loop")
        .expect("Engine globals must install");
    vm.install_scheduler_control_globals(|_| {
        Ok::<Vec<String>, std::convert::Infallible>(Vec::new())
    })
    .expect("the control globals must install");
    vm.install_coro_shims(1).expect("coro shims must install");
    if let Some(cancel) = cancel {
        vm.set_cancel(cancel);
    }
    vm
}

/// The 1-based number of the first line where two texts differ. When one is
/// a prefix of the other, that is the first line past the shorter text.
fn first_differing_line(left: &str, right: &str) -> usize {
    left.lines()
        .zip(right.lines())
        .position(|(a, b)| a != b)
        .map_or_else(
            || left.lines().count().min(right.lines().count()) + 1,
            |index| index + 1,
        )
}

/// Asserts that a shim chunk name is a live repository path: that stripping
/// its `@` prefix and resolving the rest from the repository root reaches
/// the very file whose text the constant's sibling `include_str!` embedded.
///
/// A chunk name only earns its verbatim `file:line:` rendering while it
/// points at the real file, and nothing else in the crate checks it.
pub(crate) fn assert_chunk_name_resolves(constant: &str, chunk_name: &str, embedded: &str) {
    let Some(relative) = chunk_name.strip_prefix('@') else {
        panic!(
            "{constant}: required a chunk name carrying the `@` prefix PUC renders verbatim, \
             actual {chunk_name:?}"
        );
    };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(relative);
    let Ok(on_disk) = std::fs::read_to_string(&path) else {
        panic!(
            "{constant}: chunk name {chunk_name:?} does not resolve; required a readable file \
             under the repository root, actual nothing readable at {}",
            path.display()
        );
    };
    assert!(
        on_disk == embedded,
        "{constant}: chunk name {chunk_name:?} resolves to a different file than the one \
         embedded beside the constant; required identical text, actual a first difference at \
         line {} of {}",
        first_differing_line(&on_disk, embedded),
        path.display()
    );
}

/// The refusal a `pcall` caught, from the caught error's `tostring`: its
/// first line without the `runtime error: ` prefix mlua renders.
pub(crate) fn refusal_line(caught: &str) -> Option<&str> {
    caught.strip_prefix("runtime error: ")?.lines().next()
}
