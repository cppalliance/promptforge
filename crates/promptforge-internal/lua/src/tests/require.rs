//! PromptForge's `require`: library blocks run in the caller's environment
//! and coroutine, return values, caching by library identity, cycles,
//! retries after a failed load, the key passed to the resolver, the `jump`
//! refusal during a load, library prose, library errors, and the
//! library-load reports.

use super::*;

mod errors;
mod prose;
mod reports;

/// One resolver entry: the heading as written, the identity of the library
/// whose load requires it (`None` for any other call), and the library it
/// resolves to.
type Entry = (&'static str, Option<&'static str>, Library);

/// Every resolver call so far: the heading and the passed key's identity.
type Calls = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// A library under `identity` whose blocks are `sources`, each compiled
/// at the location `library {heading}` from prompt line 100, with empty
/// prose. The line keeps the chunk name `line 100` apart from the line 1
/// of the caller `program` builds.
fn library(identity: &str, heading: &str, sources: &[&str]) -> Library {
    Library {
        key: LibraryKey::new(identity.to_owned()),
        heading: heading.to_owned(),
        blocks: sources
            .iter()
            .map(|source| LibraryBlock {
                program: LuaProgram::compile(
                    source,
                    &format!("library {heading}"),
                    NonZeroU32::new(100).expect("100 is non-zero"),
                    &null_emitter(),
                    TEST_SECTION,
                )
                .expect("library Lua must compile"),
                prose: String::new(),
            })
            .collect(),
    }
}

/// A resolver over fixed entries that records every call. A heading
/// resolves to the entry for the passed key first and to the entry with
/// no key otherwise; anything else is out of reach.
fn resolver(
    entries: Vec<Entry>,
    calls: &Calls,
) -> impl Fn(String, Option<LibraryKey>) -> Result<Library> + Send + Sync + 'static {
    let calls = Arc::clone(calls);
    move |heading, key| {
        let from = key.map(|key| key.identity().to_owned());
        calls
            .lock()
            .expect("the calls lock must not be poisoned")
            .push((heading.clone(), from.clone()));
        entries
            .iter()
            .find(|(written, at, _)| *written == heading && at.map(str::to_owned) == from)
            .or_else(|| {
                entries
                    .iter()
                    .find(|(written, at, _)| *written == heading && at.is_none())
            })
            .map(|(_, _, library)| library.clone())
            .ok_or_else(|| Error::Lua(format!("no section `{heading}` within reach of require")))
    }
}

/// A shim VM with `require` installed over `entries`, rendering library
/// prose verbatim and reporting through `emitter`, and the log of its
/// resolver calls.
#[track_caller]
fn require_vm(entries: Vec<Entry>, emitter: &Emitter) -> (BuiltVm, Calls) {
    require_vm_rendering(entries, |template, _| Ok(template.to_owned()), emitter)
}

/// [`require_vm`] with library prose rendered through `render`.
#[track_caller]
fn require_vm_rendering<P>(entries: Vec<Entry>, render: P, emitter: &Emitter) -> (BuiltVm, Calls)
where
    P: Fn(&str, ProseState<'_>) -> mlua::Result<String> + Send + Sync + 'static,
{
    let calls = Calls::default();
    let vm = TestVm::new().shims().build();
    vm.install_require(resolver(entries, &calls), render, emitter, TEST_SECTION)
        .expect("require must install");
    (vm, calls)
}

/// Runs `source` as a block coroutine to its end and returns its scalar.
fn returned(vm: &SectionVm, source: &str) -> Option<String> {
    match vm.start_block_coro(&program(source)) {
        Ok(CoroStep::Done(LuaBlockResult::Returned(value))) => value,
        other => panic!("the block must return, got {other:?}"),
    }
}

/// Runs `source` as a block coroutine and returns its failure's display.
fn failure(vm: &SectionVm, source: &str) -> String {
    match vm.start_block_coro(&program(source)) {
        Err(error) => error.to_string(),
        other => panic!("the block must fail, got {other:?}"),
    }
}

#[test]
fn require_hands_back_the_first_returned_value_of_any_type() {
    let (vm, _) = require_vm(
        vec![
            (
                "## table",
                None,
                library("t", "## table", &["return { name = 'tbl' }"]),
            ),
            (
                "## function",
                None,
                library("f", "## function", &["return function() return 'fn' end"]),
            ),
            (
                "## string",
                None,
                library("s", "## string", &["return 'text'"]),
            ),
            (
                "## false",
                None,
                library("n", "## false", &["return false"]),
            ),
        ],
        &null_emitter(),
    );
    let value = returned(
        &vm,
        "local t = require '## table'\n\
         local f = require '## function'\n\
         return type(t) .. ':' .. t.name .. '|' .. f() .. '|' .. (require '## string') .. '|' \
           .. tostring(require '## false')",
    );
    assert_eq!(value.as_deref(), Some("table:tbl|fn|text|false"));
}

#[test]
fn a_block_without_a_value_lets_the_next_block_run() {
    let (vm, _) = require_vm(
        vec![
            (
                "## later",
                None,
                library(
                    "later",
                    "## later",
                    &[
                        "first = 'ran'",
                        "return nil",
                        "return first .. ' then third'",
                    ],
                ),
            ),
            (
                "## silent",
                None,
                library("silent", "## silent", &["quiet = 1", "return nil"]),
            ),
        ],
        &null_emitter(),
    );
    let value = returned(
        &vm,
        "return (require '## later') .. '|' .. tostring(require '## silent')",
    );
    assert_eq!(value.as_deref(), Some("ran then third|true"));
}

#[test]
fn library_globals_reach_the_caller_and_its_locals_stay_inside() {
    let (vm, _) = require_vm(
        vec![(
            "## a",
            None,
            library(
                "a",
                "## a",
                &["local hidden = 'local'\nexported = 'global'"],
            ),
        )],
        &null_emitter(),
    );
    let value = returned(
        &vm,
        "require '## a'\nreturn exported .. ':' .. tostring(hidden)",
    );
    assert_eq!(value.as_deref(), Some("global:nil"));
}

#[test]
fn a_second_require_returns_the_saved_value_without_rerunning() {
    let (vm, _) = require_vm(
        vec![(
            "## a",
            None,
            library(
                "a",
                "## a",
                &["loads = (loads or 0) + 1\nreturn { count = loads }"],
            ),
        )],
        &null_emitter(),
    );
    let value = returned(
        &vm,
        "local first = require '## a'\n\
         local second = require '## a'\n\
         return tostring(first == second) .. ':' .. loads",
    );
    assert_eq!(value.as_deref(), Some("true:1"));
}

#[test]
fn a_saved_false_comes_back_without_a_rerun() {
    let (vm, _) = require_vm(
        vec![(
            "## a",
            None,
            library("a", "## a", &["loads = (loads or 0) + 1\nreturn false"]),
        )],
        &null_emitter(),
    );
    let value = returned(
        &vm,
        "local first, second = require '## a', require '## a'\n\
         return tostring(first) .. ':' .. tostring(second) .. ':' .. loads",
    );
    assert_eq!(value.as_deref(), Some("false:false:1"));
}

#[test]
fn a_library_requiring_a_loaded_library_gets_its_saved_value() {
    let (vm, _) = require_vm(
        vec![
            (
                "## a",
                None,
                library(
                    "a",
                    "## a",
                    &["loads = (loads or 0) + 1\nreturn 'value of a'"],
                ),
            ),
            (
                "## b",
                None,
                library("b", "## b", &["return 'b sees ' .. require '## a'"]),
            ),
        ],
        &null_emitter(),
    );
    let value = returned(
        &vm,
        "require '## a'\nreturn (require '## b') .. ':' .. loads",
    );
    assert_eq!(value.as_deref(), Some("b sees value of a:1"));
}

#[test]
fn nested_helpers_sharing_a_heading_stay_apart_per_library() {
    let (vm, _) = require_vm(
        vec![
            (
                "## a",
                None,
                library("a", "## a", &["return require '### helpers'"]),
            ),
            (
                "## b",
                None,
                library("b", "## b", &["return require '### helpers'"]),
            ),
            (
                "### helpers",
                Some("a"),
                library("a/helpers", "### helpers", &["return 'helper of a'"]),
            ),
            (
                "### helpers",
                Some("b"),
                library("b/helpers", "### helpers", &["return 'helper of b'"]),
            ),
        ],
        &null_emitter(),
    );
    let value = returned(&vm, "return (require '## a') .. '|' .. (require '## b')");
    assert_eq!(value.as_deref(), Some("helper of a|helper of b"));
}

#[test]
fn a_two_library_cycle_raises_naming_both_sections_in_order() {
    let (vm, _) = require_vm(
        vec![
            ("## a", None, library("a", "## a", &["require '## b'"])),
            ("## b", None, library("b", "## b", &["require '## a'"])),
        ],
        &null_emitter(),
    );
    let error = failure(&vm, "require '## a'");
    assert!(
        error.contains("require cycle: ## a -> ## b -> ## a"),
        "the cycle must name both sections in order, got {error}"
    );
}

#[test]
fn a_failed_load_stays_out_of_the_cache_and_runs_again() {
    let (vm, _) = require_vm(
        vec![(
            "## a",
            None,
            library(
                "a",
                "## a",
                &[
                    "runs = (runs or 0) + 1\npartial = 'kept'\nif broken then error('library fault') end\nreturn 'loaded'",
                ],
            ),
        )],
        &null_emitter(),
    );
    let value = returned(
        &vm,
        "broken = true\n\
         local ok, err = pcall(require, '## a')\n\
         local kept = partial\n\
         broken = false\n\
         return tostring(ok) .. '|' .. tostring(err) .. '|' .. kept .. '|' .. (require '## a') \
           .. '|' .. runs",
    );
    assert_eq!(
        value.as_deref(),
        Some("false|[string \"line 100\"]:3: library fault|kept|loaded|2")
    );
}

#[test]
fn an_uncaught_library_failure_keeps_the_library_position() {
    let (vm, _) = require_vm(
        vec![(
            "## a",
            None,
            library("a", "## a", &["\nerror('library fault')"]),
        )],
        &null_emitter(),
    );
    let error = failure(&vm, "require '## a'");
    assert!(
        error.contains("library ## a:101: library fault"),
        "the raise must still name the library block, got {error}"
    );
}

#[test]
fn only_a_require_during_a_load_passes_the_loading_library_key() {
    let (vm, calls) = require_vm(
        vec![
            (
                "## a",
                None,
                library(
                    "a",
                    "## a",
                    &["require '### helpers'\nfunction later() return require '### late' end"],
                ),
            ),
            (
                "### helpers",
                Some("a"),
                library("a/helpers", "### helpers", &["return 'h'"]),
            ),
            (
                "### late",
                None,
                library("late", "### late", &["return 'l'"]),
            ),
        ],
        &null_emitter(),
    );
    assert_eq!(
        returned(&vm, "require '## a'\nreturn later()").as_deref(),
        Some("l")
    );
    assert_eq!(
        *calls.lock().expect("the calls lock must not be poisoned"),
        [
            ("## a".to_owned(), None),
            ("### helpers".to_owned(), Some("a".to_owned())),
            ("### late".to_owned(), None),
        ]
    );
}

#[test]
fn a_non_string_heading_raises_without_reaching_the_resolver() {
    let (vm, calls) = require_vm(Vec::new(), &null_emitter());
    let value = returned(
        &vm,
        "local ok, err = pcall(require, 42)\n\
         return tostring(ok) .. '|' .. tostring(err)",
    )
    .expect("the block must return its text");
    assert!(
        value.starts_with("false|")
            && value.contains(
                "require takes a section heading string such as '## helpers', got integer"
            ),
        "a number heading must raise the heading-type error, got {value}"
    );
    assert!(
        calls
            .lock()
            .expect("the calls lock must not be poisoned")
            .is_empty(),
        "a number heading must never reach the resolver"
    );
}

#[test]
fn jump_during_a_load_raises_and_the_refusal_ends_with_the_load() {
    let (vm, _) = require_vm(
        vec![("## a", None, library("a", "## a", &["jump '## elsewhere'"]))],
        &null_emitter(),
    );
    let error = failure(&vm, "require '## a'");
    assert!(
        error.contains("jump is unavailable while a library loads"),
        "jump must refuse during a load, got {error}"
    );
    let step = vm
        .start_block_coro(&program("pcall(require, '## a')\njump '## after'"))
        .expect("the jump after a failed load must transfer");
    assert!(
        matches!(step, CoroStep::Done(LuaBlockResult::Jump(ref heading)) if heading == "## after"),
        "a jump after the load must transfer, got {step:?}"
    );
}

#[test]
fn a_library_function_that_jumps_after_the_load_jumps_for_the_caller() {
    let (vm, _) = require_vm(
        vec![(
            "## a",
            None,
            library("a", "## a", &["function go() jump '## next' end"]),
        )],
        &null_emitter(),
    );
    let step = vm
        .start_block_coro(&program("require '## a'\ngo()"))
        .expect("the jump must transfer");
    assert!(
        matches!(step, CoroStep::Done(LuaBlockResult::Jump(ref heading)) if heading == "## next"),
        "the library function's jump must transfer the caller, got {step:?}"
    );
}

#[test]
fn a_suspending_library_call_yields_from_the_callers_coroutine() {
    let (vm, _) = require_vm(
        vec![(
            "## a",
            None,
            library("a", "## a", &["answer = call('## worker', 'job')"]),
        )],
        &null_emitter(),
    );
    let block = program("require '## a'\nreturn answer");
    let CoroStep::Yielded(thread, values) = vm.start_block_coro(&block).expect("the block starts")
    else {
        panic!("the library's call must suspend the caller's block");
    };
    match vm.request_from_yield(&values) {
        YieldParse::Request(Request::Call { target, input, .. }) => {
            assert_eq!(target, "## worker");
            assert_eq!(input.as_deref(), Some("job"));
        }
        other => panic!("expected a call request, got {other:?}"),
    }
    let step = vm
        .resume_block_coro(&block, &thread, (true, "worker said hi"))
        .expect("resuming finishes the load");
    assert!(
        matches!(step, CoroStep::Done(LuaBlockResult::Returned(Some(ref text))) if text == "worker said hi"),
        "the load must finish and the block return, got {step:?}"
    );
}
