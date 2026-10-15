//! `prompt.section` and `prompt.list`: what each returns over the caller's
//! visible set, the only calls that raise, and the reads from H1 and a
//! library.

use super::*;

/// The raw Markdown `## Rules` holds: an indented first line, inline markup,
/// a `lua` fence, a thematic break, and a template, all of which
/// `prompt.section` returns verbatim, neither run nor rendered. The break
/// resets the section's prose and list capture but not its Markdown.
const RULES: &str = "  Indented first line.\n\n\
- **Bold** and `code` stay raw.\n\n\
```lua\n\
var.never = 'run'\n\
```\n\n\
---\n\n\
Closing line with {{ args }}.";

/// The sections every test joins after its own `## Main`: a child list
/// `### Kid` with a grandchild `#### Grand`, a numbered `## Nums`, the raw
/// `## Rules`, an empty `## Empty`, and `## Other` with a niece `### Niece`.
/// It stays literal because `## Rules` must come back byte for byte.
const DOCUMENT: [&str; 3] = [
    "\n### Kid\n\n\
- kid-one\n\
- kid-two\n\n\
#### Grand\n\n\
- grand-item\n\n\
## Nums\n\n\
1. one\n\
2. two\n\
3. three\n\n\
## Rules\n\n",
    RULES,
    "\n\n\
## Empty\n\n\
## Other\n\n\
Other prose.\n\n\
### Niece\n\n\
- niece-item\n",
];

/// Both reads search only the caller's visible set (its siblings minus
/// itself, plus its direct children): `prompt.section` returns a visible
/// section's own raw Markdown, `prompt.list` its pre-parsed items in order,
/// and a niece, a grandchild, the caller itself, and a heading no section
/// has all read as `nil` from both.
#[test]
fn prompt_reads_resolve_over_the_visible_set() {
    let main = prompt(
        "",
        &[(
            "Main",
            "
            assert(prompt.section('### Kid') == '- kid-one\\n- kid-two', 'a child reads its own Markdown, without its children')
            local kids = prompt.list('### Kid')
            assert(#kids == 2 and kids[1] == 'kid-one' and kids[2] == 'kid-two', 'a child list reads its items')
            local nums = prompt.list('## Nums')
            assert(#nums == 3 and nums[1] == 'one' and nums[2] == 'two' and nums[3] == 'three', 'a sibling list reads its items in order')
            assert(prompt.section('## Nums') == '1. one\\n2. two\\n3. three', 'a list section reads its raw Markdown too')
            assert(prompt.section('## Empty') == '', 'an empty section reads as the empty string')
            for _, heading in ipairs({'### Niece', '#### Grand', '## Main', '## Missing'}) do
              assert(prompt.section(heading) == nil, heading .. ' must read nil from prompt.section')
              assert(prompt.list(heading) == nil, heading .. ' must read nil from prompt.list')
            end
            return prompt.section('## Rules')",
        )],
    );
    let trace = Scenario::new(main + &DOCUMENT.concat()).run();
    assert_eq!(
        trace.text(),
        RULES,
        "a section's Markdown comes back verbatim"
    );
}

/// A read raises only when the call itself is wrong: a target that is not a
/// string, a malformed heading, or `prompt.list` naming a section with no
/// pre-parsed items, whose message names the trimmed heading. A heading no
/// section has and a heading outside the visible set are not mistakes, so
/// both read as `nil`. A `call` to a heading no section has still raises,
/// and its listing of the available sections names only the visible set.
#[test]
fn prompt_reads_raise_only_for_mistakes_in_the_call() {
    let main = prompt(
        "",
        &[(
            "Main",
            "
            local function expect_raise(read, target, needle)
              local ok, err = pcall(read, target)
              assert(not ok, 'expected a raise for ' .. tostring(target))
              assert(tostring(err):find(needle, 1, true), tostring(target) .. ' must raise with ' .. needle .. ', got: ' .. tostring(err))
            end
            for _, read in ipairs({prompt.section, prompt.list}) do
              expect_raise(read, 42, 'section target must be a string, got integer')
              expect_raise(read, 'Nums', 'got bare name: Nums')
              expect_raise(read, '##Nums', 'must have whitespace after the ## markers')
              for _, heading in ipairs({'## Missing', '### Niece'}) do
                local ok, value = pcall(read, heading)
                assert(ok and value == nil, heading .. ' must read as nil, not raise: ' .. tostring(value))
              end
            end
            expect_raise(prompt.list, '  ## Rules  ', 'section `## Rules` has no pre-parsed items')
            expect_raise(prompt.list, '## Empty', 'section `## Empty` has no pre-parsed items')
            assert(prompt.section('## Rules') ~= nil, 'prompt.section reads a section prompt.list refuses')
            local ok, err = pcall(call, '## Missing')
            assert(not ok, 'a call to a missing heading still raises')
            local message = tostring(err)
            local at = message:find('available sections:', 1, true)
            assert(at, 'the call error lists the available sections: ' .. message)
            local listed = message:sub(at)
            for _, heading in ipairs({'## Other', '### Kid'}) do
              assert(listed:find(heading, 1, true), 'the listing names the visible ' .. heading .. ': ' .. message)
            end
            for _, hidden in ipairs({'## Main', 'Niece', 'Grand'}) do
              assert(not listed:find(hidden, 1, true), 'the listing leaks ' .. hidden .. ': ' .. message)
            end
            return 'ok'",
        )],
    );
    let trace = Scenario::new(main + &DOCUMENT.concat()).run();
    assert_eq!(
        trace.text(),
        "ok",
        "every raise must be caught and every miss must read nil"
    );
}

/// Both reads work in H1, whose visible set is the whole top-level slice
/// (every top-level section and no child), and in a library, which reads
/// from the calling section: at load time, where each requiring section's
/// load resolves over that section's own visible set, and from a library
/// function, which resolves over the visible set of whichever section
/// calls it.
#[test]
fn prompt_reads_work_in_h1_and_a_library() {
    let head = PromptSource::new()
        .h1(
            "
            require '## kid_reader'
            assert(prompt.section('## Main') ~= nil, 'H1 sees every top-level section')
            assert(prompt.section('### Kid') == nil, 'H1 sees no child')
            assert(kids_at_load == nil, 'the load-time read in the H1 resolves over the H1 visible set')
            assert(kid_items() == nil, 'a library function reads over the H1 visible set')
            var.nums = table.concat(prompt.list('## Nums'), ',')",
        )
        .section(
            "Main",
            "
            require '## kid_reader'
            assert(kids_at_load ~= nil, 'the load-time read in Main resolves over the Main visible set')
            return var.nums .. '|' .. table.concat(kids_at_load, ',') .. '|' .. table.concat(kid_items(), ',')",
        );
    let library = "\n## kid_reader\n\n\
```lua\n\
kids_at_load = prompt.list('### Kid')\n\
function kid_items()\n\
  return prompt.list('### Kid')\n\
end\n\
```\n";
    let source = [head.build().as_str(), &DOCUMENT.concat(), library].concat();
    let trace = Scenario::new(source).run();
    let out = trace.text();
    assert_eq!(
        out, "one,two,three|kid-one,kid-two|kid-one,kid-two",
        "H1 reads the top-level list, and both the load-time read and the library function read the caller's child"
    );
}
