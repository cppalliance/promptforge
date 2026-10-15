//! Line-mapping tests: a runtime error in each Lua block reports its
//! file-absolute line.

use super::*;

fn assert_runtime_error_line(program: &LuaProgram, absolute_line: u32) {
    let lua = mlua::Lua::new();
    let function = program.load(&lua).expect("bytecode must load");
    let raw_error = function
        .call::<()>(())
        .expect_err("assert(false) must fail");
    let mapped = program.map_runtime_error(&raw_error);
    let msg = mapped.to_string();
    assert!(
        msg.contains(&format!(":{absolute_line}:")),
        "error must contain absolute line {absolute_line}: {msg}"
    );
}

#[test]
fn epilog_source_line_maps_runtime_error_to_absolute_line() {
    // Lines:
    //  1: ---
    //  2: name: x
    //  3: description: d
    //  4: ---
    //  5: (empty)
    //  6: # T
    //  7: (empty)
    //  8: ## Check
    //  9: (empty)
    // 10: Ask the model.
    // 11: (empty)
    // 12: ```lua       <- epilog opens
    // 13: local a = 1  <- epilog line 1 (source_line = 13)
    // 14: assert(false) <- epilog line 2 (absolute = 14)
    // 15: ```
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Check\n\nAsk the model.\n\n```lua\nlocal a = 1\nassert(false)\n```\n";
    let prompt = parse_ok(src);
    let epilog = prompt
        .entry()
        .expect("has sections")
        .epilog()
        .expect("epilog must exist");

    assert_eq!(
        epilog.source_line().get(),
        13,
        "epilog Lua starts on line 13"
    );
    assert_eq!(epilog.source(), "local a = 1\nassert(false)");

    // Simulate a runtime error: assert(false) is on chunk line 2.
    // Absolute line = 13 + 2 - 1 = 14.
    assert_runtime_error_line(epilog, 14);
}

#[test]
fn prologue_source_line_maps_correctly() {
    // Lines:
    //  1: ---
    //  2: name: x
    //  3: description: d
    //  4: ---
    //  5: (empty)
    //  6: # T
    //  7: (empty)
    //  8: ## Work
    //  9: (empty)
    // 10: ```lua       <- prologue opens
    // 11: assert(false) <- prologue line 1 (source_line = 11, absolute = 11)
    // 12: ```
    // 13: (empty)
    // 14: Do the work.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Work\n\n```lua\nassert(false)\n```\n\nDo the work.\n";
    let prompt = parse_ok(src);
    let prologue = prompt
        .entry()
        .expect("has sections")
        .prologue()
        .expect("prologue must exist");

    assert_eq!(
        prologue.source_line().get(),
        11,
        "prologue Lua starts on line 11"
    );

    assert_runtime_error_line(prologue, 11);
}

#[test]
fn multi_line_chunk_maps_inner_line_correctly() {
    // Epilog with assert on line 3 of the fence.
    //  1-4: frontmatter
    //  5: empty
    //  6: # T
    //  7: empty
    //  8: ## S
    //  9: empty
    // 10: Prose.
    // 11: empty
    // 12: ```lua
    // 13: local x = 1    <- source_line = 13
    // 14: local y = 2
    // 15: assert(false)  <- chunk line 3, absolute = 13 + 3 - 1 = 15
    // 16: ```
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nProse.\n\n```lua\nlocal x = 1\nlocal y = 2\nassert(false)\n```\n";
    let prompt = parse_ok(src);
    let epilog = prompt
        .entry()
        .expect("has sections")
        .epilog()
        .expect("epilog must exist");

    assert_eq!(epilog.source_line().get(), 13);

    assert_runtime_error_line(epilog, 15);
}

#[test]
fn every_block_of_a_section_maps_only_its_own_errors() {
    //  8: ## S
    // 10: ```lua         <- prologue
    // 16: ```lua         <- second block
    // 17: assert(false)
    // 22: ```lua         <- third block (source_line = 23)
    // 23: local y = 2
    // 24: assert(false)  <- chunk line 2, absolute = 24
    // 29: ```lua         <- epilog
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n\
         ## S\n\n```lua\nx = 1\n```\n\nOne.\n\n```lua\nassert(false)\n```\n\nTwo.\n\n\
         ```lua\nlocal y = 2\nassert(false)\n```\n\nThree.\n\n```lua\nz = 3\n```\n";
    let prompt = parse_ok(src);
    let section = prompt.entry().expect("has sections");
    let programs: Vec<&LuaProgram> = section
        .blocks()
        .iter()
        .filter_map(|block| match block {
            Block::Lua(program) => Some(program),
            Block::Prose { .. } => None,
        })
        .collect();
    let locations: std::collections::BTreeSet<&str> =
        programs.iter().map(|program| program.location()).collect();
    assert_eq!(
        locations.len(),
        4,
        "each of the four blocks needs its own chunk name, got {locations:?}"
    );
    let (second, third) = (programs[1], programs[2]);
    assert_eq!(third.source_line().get(), 23);
    assert_runtime_error_line(third, 24);

    let lua = mlua::Lua::new();
    let raw_error = third
        .load(&lua)
        .expect("bytecode must load")
        .call::<()>(())
        .expect_err("assert(false) must fail");
    assert_eq!(
        second.map_runtime_error(&raw_error).to_string(),
        raw_error.to_string(),
        "another block's program must leave the third block's error unmapped"
    );
}

#[test]
fn sections_sharing_a_title_get_locations_named_by_heading_line() {
    // 10: ### Helpers    <- under `A`
    // 12: ```lua         <- source_line = 13
    // 18: ### Helpers    <- under `B`
    // 20: ```lua         <- source_line = 21
    // 22: assert(false)  <- chunk line 2, absolute = 22
    // 25: ## C           <- a title no other section uses
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n\
         ## A\n\n### Helpers\n\n```lua\nassert(false)\n```\n\n\
         ## B\n\n### Helpers\n\n```lua\nlocal pad = 1\nassert(false)\n```\n\n\
         ## C\n\n```lua\nx = 1\n```\n";
    let prompt = parse_ok(src);
    let only_program = |section: &Section| match section.blocks() {
        [Block::Lua(program)] => program.clone(),
        other => panic!("one Lua block expected, got {other:?}"),
    };
    let under_a = only_program(&prompt.sections[0].children()[0]);
    let under_b = only_program(&prompt.sections[1].children()[0]);
    assert_eq!(under_a.location(), "section `Helpers` (line 10) prologue");
    assert_eq!(under_b.location(), "section `Helpers` (line 18) prologue");
    assert_eq!(
        only_program(&prompt.sections[2]).location(),
        "section `C` prologue"
    );
    assert_runtime_error_line(&under_b, 22);

    let lua = mlua::Lua::new();
    let raw_error = under_b
        .load(&lua)
        .expect("bytecode must load")
        .call::<()>(())
        .expect_err("assert(false) must fail");
    assert_eq!(
        under_a.map_runtime_error(&raw_error).to_string(),
        raw_error.to_string(),
        "the other `Helpers` must leave this one's error unmapped"
    );
}

#[test]
fn every_h1_block_maps_only_its_own_errors() {
    //  8: ```lua         <- first block (source_line = 9)
    //  9: assert(false)
    // 12: Plan.
    // 14: ```lua         <- second block (source_line = 15)
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nassert(false)\n```\n\nPlan.\n\n```lua\nx = 1\n```\n\n## S\n\np\n";
    let prompt = parse_ok(src);
    let [Block::Lua(first), Block::Prose { .. }, Block::Lua(second)] = prompt.h1_blocks.as_slice()
    else {
        panic!("H1 Lua, prose, Lua expected, got {:?}", prompt.h1_blocks);
    };
    assert_eq!(second.source_line().get(), 15);
    assert_runtime_error_line(first, 9);

    let lua = mlua::Lua::new();
    let raw_error = first
        .load(&lua)
        .expect("bytecode must load")
        .call::<()>(())
        .expect_err("assert(false) must fail");
    assert_eq!(
        second.map_runtime_error(&raw_error).to_string(),
        raw_error.to_string(),
        "the second H1 block must leave the first block's error unmapped"
    );
}

#[test]
fn a_long_unique_section_title_maps_its_prologue_line() {
    //  8: ## Gather Background Research Notes
    // 10: ```lua         <- prologue opens
    // 11: assert(false)  <- source_line = 11, absolute = 11
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Gather Background Research Notes\n\n```lua\nassert(false)\n```\n\nGo.\n";
    let prompt = parse_ok(src);
    let prologue = prompt
        .entry()
        .expect("has sections")
        .prologue()
        .expect("prologue must exist");
    assert_eq!(
        prologue.location(),
        "section `Gather Background Research Notes` prologue"
    );
    assert_runtime_error_line(prologue, 11);
}

#[test]
fn a_shared_title_whose_heading_line_reaches_100_maps_its_prologue_line() {
    //  10: ### Research Worker Notes  <- under `A`
    //  13: assert(false)              <- under `A` source_line = 13
    //  16-105: Padding.
    // 109: ### Research Worker Notes  <- under `B`
    // 112: local pad = 1              <- under `B` source_line = 112
    // 113: assert(false)              <- chunk line 2, absolute = 113
    let src = format!(
        "---\nname: x\ndescription: d\n---\n\n# T\n\n\
         ## A\n\n### Research Worker Notes\n\n```lua\nassert(false)\n```\n\n{}\n\
         ## B\n\n### Research Worker Notes\n\n```lua\nlocal pad = 1\nassert(false)\n```\n",
        "Padding.\n".repeat(90),
    );
    let prompt = parse_ok(&src);
    let prologue = |section: &Section| section.prologue().expect("prologue must exist").clone();
    let under_a = prologue(&prompt.sections[0].children()[0]);
    let under_b = prologue(&prompt.sections[1].children()[0]);
    assert_eq!(
        under_b.location(),
        "section `Research Worker Notes` (line 109) prologue"
    );
    assert_eq!(under_b.source_line().get(), 112);
    assert_runtime_error_line(&under_b, 113);

    let lua = mlua::Lua::new();
    let raw_error = under_b
        .load(&lua)
        .expect("bytecode must load")
        .call::<()>(())
        .expect_err("assert(false) must fail");
    assert_eq!(
        under_a.map_runtime_error(&raw_error).to_string(),
        raw_error.to_string(),
        "the other `Research Worker Notes` must leave this one's error unmapped"
    );
}

#[test]
fn unrecognized_lua_label_error_reports_the_fence_opening_line() {
    // Lines:
    //  6: # T
    //  8: ## S
    // 10: Prose.
    // 12: ```lua extra <- the error points here
    // 13: function f()
    // 14: end
    // 15: ```
    // 17: ```Lua       <- a later offender, not reported
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n\
         ## S\n\nProse.\n\n```lua extra\nfunction f()\nend\n```\n\n```Lua\nreturn 1\n```\n";
    let error = parse_err(src);
    assert_eq!(error.kind(), ParseErrorKind::Fence, "{error}");
    assert_eq!(error.line(), Some(12), "{error}");
    assert_eq!(error.column(), Some(1), "{error}");
    assert!(error.span().is_some(), "{error}");
}
