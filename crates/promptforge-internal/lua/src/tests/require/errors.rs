//! Library errors: a raise in library code, during its load or from a
//! library function called later, reports the library block's location and
//! prompt-file line, and a message holding several chunks' markers maps
//! each one once under a single leading tag.

use super::*;

/// A program compiled under `location`, its first line at prompt line
/// `line`.
fn at(source: &str, location: &str, line: u32) -> LuaProgram {
    LuaProgram::compile(
        source,
        location,
        NonZeroU32::new(line).expect("prompt lines start at 1"),
        &null_emitter(),
        SECTION,
    )
    .expect("test Lua must compile")
}

/// A library under `identity` whose blocks are `blocks`, each a source
/// with its location and first prompt line.
fn placed(identity: &str, heading: &str, blocks: &[(&str, &str, u32)]) -> Library {
    Library {
        key: LibraryKey::new(identity.to_owned()),
        heading: heading.to_owned(),
        blocks: blocks
            .iter()
            .map(|(source, location, line)| LibraryBlock {
                program: at(source, location, *line),
                prose: String::new(),
            })
            .collect(),
    }
}

/// Runs `program` as a block coroutine and returns its failure's display.
fn failing(vm: &SectionVm, program: &LuaProgram) -> String {
    match vm.start_block_coro(program) {
        Err(error) => error.to_string(),
        other => panic!("the block must fail, got {other:?}"),
    }
}

#[test]
fn a_raise_during_the_load_and_from_a_later_call_reports_the_library_line() {
    let (vm, _) = require_vm(
        vec![(
            "## lib",
            None,
            placed(
                "lib",
                "## lib",
                &[(
                    "function boom()\n  error('later fault')\nend\n\
                     if fail_now then error('load fault') end",
                    "section `lib` prologue",
                    40,
                )],
            ),
        )],
        &null_emitter(),
    );
    let during = failing(&vm, &program("fail_now = true\nrequire '## lib'"));
    assert!(
        during.starts_with("section `lib` prologue:43: ")
            && during.contains("section `lib` prologue:43: load fault"),
        "a raise during the load must report the library's line 43, got {during}"
    );
    let later = failing(&vm, &program("fail_now = false\nrequire '## lib'\nboom()"));
    assert!(
        later.starts_with("section `lib` prologue:41: ")
            && later.contains("section `lib` prologue:41: later fault"),
        "a raise from a library function must report the library's line 41, got {later}"
    );
}

#[test]
fn a_raise_in_the_second_of_two_middle_blocks_maps_to_its_own_line() {
    let (vm, _) = require_vm(
        vec![(
            "## lib",
            None,
            placed(
                "lib",
                "## lib",
                &[
                    ("first = 1", "section `lib` lua 2", 10),
                    (
                        "local seen = first\nerror('second fault')",
                        "section `lib` lua 3",
                        20,
                    ),
                ],
            ),
        )],
        &null_emitter(),
    );
    let error = failing(&vm, &program("require '## lib'"));
    assert!(
        error.starts_with("section `lib` lua 3:21: ")
            && error.contains("section `lib` lua 3:21: second fault"),
        "the second middle block's raise must map to its own line 21, got {error}"
    );
}

#[test]
fn library_and_caller_markers_each_map_once_under_one_leading_tag() {
    let (vm, _) = require_vm(
        vec![(
            "## lib",
            None,
            placed(
                "lib",
                "## lib",
                &[(
                    "function boom()\n  error('deep fault')\nend",
                    "section `lib` prologue",
                    50,
                )],
            ),
        )],
        &null_emitter(),
    );
    let caller = at("require '## lib'\nboom()", "section `Main` prologue", 30);
    let error = failing(&vm, &caller);
    assert!(
        error.starts_with("section `lib` prologue:51: "),
        "the leading tag must name the raise point, got {error}"
    );
    assert_eq!(
        error.lines().next(),
        Some("section `lib` prologue:51: runtime error: section `lib` prologue:51: deep fault"),
        "exactly one leading tag before the raise point, got {error}"
    );
    for mapped in [
        "section `lib` prologue:51: deep fault",
        "section `lib` prologue:51: in",
        "section `Main` prologue:31: in",
    ] {
        assert!(
            error.contains(mapped),
            "each marker must map once to its own prompt line ({mapped}), got {error}"
        );
    }
    assert!(
        !error.contains("[string \""),
        "no marker may keep its chunk line, got {error}"
    );
}

#[test]
fn a_parent_that_loaded_the_same_library_keeps_a_childs_mapped_line() {
    let lib = || {
        placed(
            "lib",
            "## lib",
            &[(
                "function boom()\n  error('deep fault')\nend",
                "section `lib` prologue",
                50,
            )],
        )
    };
    let (child, _) = require_vm(vec![("## lib", None, lib())], &null_emitter());
    let surfaced = failing(&child, &program("require '## lib'\nboom()"));
    assert!(
        surfaced.contains("section `lib` prologue:51: deep fault"),
        "the child must map the library line first, got {surfaced}"
    );
    let (parent, _) = require_vm(vec![("## lib", None, lib())], &null_emitter());
    let error = failing(
        &parent,
        &program(&format!("require '## lib'\nerror([==[{surfaced}]==], 0)")),
    );
    assert!(
        error.contains(&surfaced),
        "the parent must leave the child's mapped message intact, got {error}"
    );
    assert!(
        !error.contains(":100:"),
        "line 51 must not map through the library a second time, got {error}"
    );
}
