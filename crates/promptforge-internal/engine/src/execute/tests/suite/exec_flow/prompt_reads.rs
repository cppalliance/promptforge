//! `prompt.section` and `prompt.list`: what each returns over the caller's
//! visible set, the only calls that raise, and the reads from H1 and the
//! shared library.

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
const DOCUMENT: [&str; 3] = [
    "### Kid\n\n\
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
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_reads_resolve_over_the_visible_set() {
    let md = [
        flow_prompt!(
            "\
## Main\n\n\
```lua\n\
assert(prompt.section('### Kid') == '- kid-one\\n- kid-two', 'a child reads its own Markdown, without its children')\n\
local kids = prompt.list('### Kid')\n\
assert(#kids == 2 and kids[1] == 'kid-one' and kids[2] == 'kid-two', 'a child list reads its items')\n\
local nums = prompt.list('## Nums')\n\
assert(#nums == 3 and nums[1] == 'one' and nums[2] == 'two' and nums[3] == 'three', 'a sibling list reads its items in order')\n\
assert(prompt.section('## Nums') == '1. one\\n2. two\\n3. three', 'a list section reads its raw Markdown too')\n\
assert(prompt.section('## Empty') == '', 'an empty section reads as the empty string')\n\
for _, heading in ipairs({'### Niece', '#### Grand', '## Main', '## Missing'}) do\n\
  assert(prompt.section(heading) == nil, heading .. ' must read nil from prompt.section')\n\
  assert(prompt.list(heading) == nil, heading .. ' must read nil from prompt.list')\n\
end\n\
return prompt.section('## Rules')\n\
```\n\n"
        ),
        DOCUMENT.concat().as_str(),
    ]
    .concat();
    let out = run_offline(&md)
        .await
        .expect("both reads must resolve over the visible set");
    assert_eq!(out, RULES, "a section's Markdown comes back verbatim");
}

/// A read raises only when the call itself is wrong: a target that is not a
/// string, a malformed heading, or `prompt.list` naming a section with no
/// pre-parsed items, whose message names the trimmed heading. A heading no
/// section has and a heading outside the visible set are not mistakes, so
/// both read as `nil`. A `call` to a heading no section has still raises,
/// and its listing of the available sections names only the visible set.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_reads_raise_only_for_mistakes_in_the_call() {
    let md = [
        flow_prompt!(
            "\
## Main\n\n\
```lua\n\
local function expect_raise(read, target, needle)\n\
  local ok, err = pcall(read, target)\n\
  assert(not ok, 'expected a raise for ' .. tostring(target))\n\
  assert(tostring(err):find(needle, 1, true), tostring(target) .. ' must raise with ' .. needle .. ', got: ' .. tostring(err))\n\
end\n\
for _, read in ipairs({prompt.section, prompt.list}) do\n\
  expect_raise(read, 42, 'section target must be a string, got integer')\n\
  expect_raise(read, 'Nums', 'got bare name: Nums')\n\
  expect_raise(read, '##Nums', 'must have whitespace after the ## markers')\n\
  for _, heading in ipairs({'## Missing', '### Niece'}) do\n\
    local ok, value = pcall(read, heading)\n\
    assert(ok and value == nil, heading .. ' must read as nil, not raise: ' .. tostring(value))\n\
  end\n\
end\n\
expect_raise(prompt.list, '  ## Rules  ', 'section `## Rules` has no pre-parsed items')\n\
expect_raise(prompt.list, '## Empty', 'section `## Empty` has no pre-parsed items')\n\
assert(prompt.section('## Rules') ~= nil, 'prompt.section reads a section prompt.list refuses')\n\
local ok, err = pcall(call, '## Missing')\n\
assert(not ok, 'a call to a missing heading still raises')\n\
local message = tostring(err)\n\
local at = message:find('available sections:', 1, true)\n\
assert(at, 'the call error lists the available sections: ' .. message)\n\
local listed = message:sub(at)\n\
for _, heading in ipairs({'## Other', '### Kid'}) do\n\
  assert(listed:find(heading, 1, true), 'the listing names the visible ' .. heading .. ': ' .. message)\n\
end\n\
for _, hidden in ipairs({'## Main', 'Niece', 'Grand'}) do\n\
  assert(not listed:find(hidden, 1, true), 'the listing leaks ' .. hidden .. ': ' .. message)\n\
end\n\
return 'ok'\n\
```\n\n"
        ),
        DOCUMENT.concat().as_str(),
    ]
    .concat();
    let out = run_offline(&md)
        .await
        .expect("every raise must be caught and every miss must read nil");
    assert_eq!(out, "ok");
}

/// Both reads work in H1, whose visible set is the whole top-level slice
/// (every top-level section and no child), and in the shared library: at
/// load time, where each VM's replay resolves over that VM's own visible
/// set, and from a shared function, which resolves over the visible set of
/// whichever section calls it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_reads_work_in_h1_and_the_shared_library() {
    let md = [
        flow_prompt!(
            "\
# Test prompt\n\n\
```lua shared\n\
kids_at_load = prompt.list('### Kid')\n\
function kid_items()\n\
  return prompt.list('### Kid')\n\
end\n\
```\n\n\
```lua\n\
assert(prompt.section('## Main') ~= nil, 'H1 sees every top-level section')\n\
assert(prompt.section('### Kid') == nil, 'H1 sees no child')\n\
assert(kids_at_load == nil, 'the load-time read in the H1 replay resolves over the H1 visible set')\n\
assert(kid_items() == nil, 'a shared function reads over the H1 visible set')\n\
var.nums = table.concat(prompt.list('## Nums'), ',')\n\
```\n\n\
## Main\n\n\
```lua\n\
assert(kids_at_load ~= nil, 'the load-time read in the Main replay resolves over the Main visible set')\n\
return var.nums .. '|' .. table.concat(kids_at_load, ',') .. '|' .. table.concat(kid_items(), ',')\n\
```\n\n"
        ),
        DOCUMENT.concat().as_str(),
    ]
    .concat();
    let out = run_offline(&md)
        .await
        .expect("both reads must work in H1 and the shared library");
    assert_eq!(
        out, "one,two,three|kid-one,kid-two|kid-one,kid-two",
        "H1 reads the top-level list, and both the load-time read and the shared function read the caller's child"
    );
}
