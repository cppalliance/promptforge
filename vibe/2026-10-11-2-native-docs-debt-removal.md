---
name: native docs debt removal
overview: "Remove the debt found in the 13 unpushed runvil commits, plus the visible-set copying the user added to scope: correct the native subagents text, the PACKAGE docs, and the UnknownPackage advice; keep each prompt's body as one shared read-only string with sections as offsets into it; and read the shared prompt in place instead of copying sections."
todos:
  - id: native-text
    content: "D1-1: rewrite the NATIVE description, the PluginKind::Native doc, and the AGENTS.md Engine-native clause to say the native reserves the subagents name and offers nothing yet"
    status: completed
  - id: package-docs
    content: "CH-1: change the three PACKAGE doc lines (plugin-web, plugin-user-input, plugin-mcp) to say a Host adds the constant to its package registry"
    status: completed
  - id: unknown-advice
    content: "D1-10: change the UnknownPackage error text in host.rs to say add the package before building the HostContext"
    status: completed
  - id: visible-in-place
    content: "D1-17 part 1: replace the copied Vec<Section> visible sets with a borrowed Visible view; reads, jump, call, and fanout look sections up in the shared Arc<Prompt>; move SlicePath to walk_target.rs"
    status: completed
  - id: shared-body
    content: "D1-17 part 2: keep the parsed body as one Arc<str>; Section markdown, items, and prose become BodyText offsets into it; names stay owned; H1 prose points into its masked copy; pending prose shares instead of copying"
    status: completed
  - id: verify
    content: "Run the Testing Plan exit checks: both full nextest runs, both clippy runs, fmt, docs builds, xtask api --check, build-xtask, docs-claims, and the rg checks"
    status: completed
isProject: false
---

# Correct the Plugin docs and stop copying prompt sections

### Context for a fresh reader

- Repository: `c:\Users\Vinnie\cursor\runvil` (git), branch `master`. Every path in this plan is relative to it unless it is absolute.
- Baseline: `HEAD` is `72fc0f5bb` ("Close plan: package-registry"), 13 commits ahead of `origin/master` (`abe218a36`). Nothing is pushed. Line numbers are as of `72fc0f5bb`; search by name if they have moved.
- Terms from `AGENTS.md`: the Engine runs prompts and performs no I/O; the Harness steps the Engine and performs its effects; the Host is the application (Workshop, or a command-line Host); a Plugin is a package of tools a Host installs.
- Terms this plan uses:
  - *section*: one Markdown heading (H2 to H6) of a prompt and the text under it, up to the next heading. Sections form a tree: an H2 holds the H3s under it, and so on. The parser's `Section` type holds one, with its `children`.
  - *H1 pass*: the prompt's title section, which runs first and is not a `Section`.
  - *walk*: the Engine running sections in order, top to bottom.
  - *section entry*: the Engine starting a section. That happens for every walk step, every re-entry after a jump, every `call` child, and the first section of every task or fanout arm. Each entry builds a frame with `SectionContext::new`.
  - *`jump`, `call`, `fanout`*: Lua globals that name another section by a heading string such as `'## Retry'`. `jump` continues the walk there, `call` runs it and returns its result, and `fanout` runs it once per item.
  - *home slice*: the list of sections a section sits in, meaning it and its siblings.
  - *visible set*: the sections a running section may address by heading: its home slice minus itself, plus its own direct children. Never its parent, cousins, or grandchildren.
  - *`prompt.section(h)` and `prompt.list(h)`*: Lua reads that return the visible section `h`'s own raw markdown, or its bullet items, or nil when no visible section matches.
  - *body*: the prompt text after the frontmatter, which the parser rebuilds once with LF line endings and parses.
  - *`SlicePath`*: the scheduler's record of where a slice sits in the tree, as the index of each ancestor from the top level down. It resolves against the shared `Arc<Prompt>` on demand.
- Debt IDs (D1-1, CH-1, D1-10, D1-17) come from a debt analysis of the 13 commits. Each is defined with its evidence in the Debt Inventory below, so no other record is needed.
- Running it: execute this plan with the vibe coder (`c:\Users\Vinnie\cursor\tools-public\planning\vibe-coder.md`), whose survey fills the Project Survey section. The plan record goes in `vibe/` as `2026-10-<dd>-<n>-<words>.md`, using the next free number for that day; the latest record is `vibe/2026-10-11-1-package-registry.md`. Commit messages follow `c:\Users\Vinnie\cursor\.cursor\rules\commit.mdc` and carry a `Plan: vibe/<file>` trailer. Nothing is pushed.
- The stale user guide in the separate `runvil-docs` repository is recorded under Deferred and Out of Scope. It is not part of this run.

<product-contract>

## Product Requirements

- Scope and target work:
  - Repository `c:\Users\Vinnie\cursor\runvil`, branch `master`. The analyzed work is the 13 unpushed commits `abe218a36..72fc0f5bb`: the prompt namespace reads plan, its debt removal plan, a `vibe/` tidy-up, and the package registry plan (`vibe/2026-10-11-1-package-registry.md`, commits `9a9a18525` and `92bff15b4`).
  - Kept findings D1-1 and CH-1 come from the package registry work. The user chose to fix D1-10 from the rejected candidates.
  - The user expanded the scope to D1-17: the Engine copies sections out of the shared parsed prompt every time a section starts and every time it resolves a `jump`, `call`, or fanout target. The pattern predates the target, and `475fc39cb` made each copy larger.
- Why, in the user's words:
  - On the copies: "When the prompt is parsed, it should go into a shared string." "Why are we borrowing at all? prompt text is read-only." The parsed prompt never changes while it runs, so no part of the Engine needs its own copy of any section.
  - On what a read returns: "when someone asks for an H2, they should not get the child sections." Reads already return only the section's own text, so copying children to answer them is pure waste.
  - On the storage: "I like the idea of having one big shared string, and then computing offsets into it, to constructed a hierarchal table of sections, mirroring the markdown structure. That feels right."
  - On the native's text: `subagents` is model-facing, unlike `tasks`, which is pure Lua. So the native must eventually behave like a linked Plugin, with installing it controlling what the model gets. Until the overhaul delivers that, its text must not claim it.
- Cleanup goals:
  - Every description of the native `runvil/subagents` package says what it does today: it reserves the `subagents` name and offers nothing yet.
  - Every Plugin `PACKAGE` doc describes the install that exists: a Host adds the constant to its package registry and installs by package name.
  - The `UnknownPackage` refusal tells a Host author a step they can still take.
  - A parsed prompt stores its text once. The parser keeps the prompt body as one shared, read-only string, and every section's own text (its raw markdown, prose blocks, and list items) is a stretch of that string recorded by byte offsets. The sections still form a tree that mirrors the Markdown headings: each H2 holds its H3s, and so on.
  - A running prompt reads the shared tree in place. No section start, `prompt.section` or `prompt.list` read, `jump`, `call`, or fanout lookup copies a section.
- Non-goals:
  - Making the Host's install of `subagents` control the model's task tools. That belongs to the `tasks` and `subagents` overhaul.
  - Any change a prompt author or Host can observe. Heading addressing, read results, nil on a miss, jump, call, and fanout targets, error messages (including the ones that list the visible set), prose templates, list items, parse errors, spans, and line numbers all behave exactly as today.
  - Changing how `LuaProgram` stores its source and bytecode.
  - Any public API change. The `runvil` facade exposes neither `Section` nor `Block`, and `crates/runvil/public-api.txt` stays unchanged.
- Success criteria:
  - No text in the repository outside `vibe/` says the native lets the model start, wait for, or cancel subagents, or that the Engine implements it today.
  - No doc says a Host passes a `PACKAGE` constant to its install.
  - No Engine code builds a `Vec<Section>`. Visible sets are borrowed views over the shared prompt.
  - `Section` owns no text except its heading name, and `Block::Prose` owns none.
  - Every existing test passes and still asserts the same facts, the new tests pass, and every exit check passes. A test may change shape only where a signature or a field type changed: a call site, or the expression that builds an expected value.

## Functional Specification

### Debt Inventory

- Debt added:
  - D1-1 (introduced, `92bff15b4` and `9a9a18525`): the native package's text claims behavior it doesn't have.
    - Evidence: `NATIVE` in `crates/harness-internal/runner/src/packages.rs` line 118 has the description "Lets the model start, wait for, and cancel subagents". `Package::description`'s field doc says it is "One line saying what the Plugin offers, for a Host listing the packages it can install", and `PackageRegistry::list()` is public through `harness::plugin`. `AGENTS.md` line 12 says the native is "implemented in the Engine under the fixed name `subagents`", and `PluginKind::Native`'s doc (`packages.rs` line 37) says "what it does lives in the Engine". No Engine crate contains the string `subagent`.
    - Code facts: `Native::tools()` is empty, so installing the native adds no tool. The Engine's four task built-ins (`task`, `task_cancel`, `task_status`, `await_tasks`) are offered whenever a section calls `tools.allow_tasks()`, without consulting the Host, so leaving the native out removes nothing.
    - Impact: a Host listing packages shows a line that is false both ways. No production code reads it yet (Workshop installs the native unconditionally and keeps no registry handle), so severity is low.
    - Reversal cost: low. Text in three places, no interface change.
    - Target state: the description, the `AGENTS.md` clause, and the `PluginKind::Native` doc say the native reserves the name for Engine-implemented subagent tools that are not built yet.
  - CH-1 (introduced, `92bff15b4`; `9a9a18525` edited the declarations right below without fixing the doc): three `PACKAGE` docs describe the old install.
    - Evidence: `crates/plugin-web/src/web.rs` line 22, `crates/plugin-user-input/src/lib.rs` line 79, and `crates/plugin-mcp/src/lib.rs` line 82 each read "The Plugin's label, which a Host passes to its install." `HostContext::install` now takes a package name `&str`, and the constant goes to `PackageRegistry::add`. The package registry plan rewrote the matching sentence in the `Package` doc but missed these three.
    - Impact: a Host author following the doc gets a compile error. Low severity.
    - Target state: each says a Host adds the constant to its package registry.
  - D1-1's doc half and CH-1 share one cause: the doc sweep in `92bff15b4` was incomplete. One doc pass fixes both.
- Cheap fixes:
  - D1-10 (selected by the user from the rejected candidates; introduced in `92bff15b4`): the `UnknownPackage` message gives advice a Host cannot follow.
    - Evidence: `crates/harness-internal/runner/src/host.rs` line 246 reads "Plugin package `{package}` is not in the registry; add it with PackageRegistry::add before installing it". `HostContext::new(packages, services)` takes the registry by value and exposes no accessor, so by the time `install` refuses a name, no registry is left to call `add` on.
    - Target state: the advice says to add the package before building the `HostContext`.
- User-expanded scope (pre-existing, worsened by `475fc39cb`):
  - D1-17: the Engine copies sections out of the shared parsed prompt.
    - Storage: `Section` (`crates/runvil-internal/parser/src/prompt.rs` lines 31-45) owns `name`, `markdown`, and `items` as `String`s, owns its `blocks` (prose `String`s and `LuaProgram`s holding source and bytecode), and owns its `children`. Cloning one copies its whole subtree. The parser builds the body once (`split_frontmatter` in `build-frontmatter.rs` lines 215-246) but drops it after parsing, and every heading's content is copied out of it (`build.rs` lines 136-146).
    - Copy sites, all non-test Engine code:
      - Every section entry. `SectionContext::new` (`crates/runvil-internal/engine/src/execute/section_context-construct.rs` line 91) calls `visible_sections(siblings, section)`. That runs for every walk step, every re-entry after a jump, every `call` child, and the first section of every task or fanout arm. `home_without` (`walk_target.rs` lines 28-35) clones each sibling with its whole subtree, and `visible_sections` (lines 39-44) clones the section's direct children. The copy moves into the `prompt.section` and `prompt.list` callback whether or not the section ever reads.
      - The H1 pass. `SectionContext::new_live_h1` (line 168) copies the whole top-level slice with `.to_vec()`, once per run.
      - Every target lookup. `resolve_jump_target` (`walk_target.rs` lines 90-108) builds the same copy for every `jump`, `call`, and fanout target that `Scheduler::resolve_chain_target` resolves (`scheduler/walk.rs` line 303), searches it, and throws it away.
      - Every executed prose block. `scheduler/step.rs` line 88 copies the block's prose `String` into the chain's `pending_prose`.
    - A read only ever returns the matched section's own markdown and list items (`read_visible`, `walk_target.rs` lines 59-66). Children are never part of a read result, so copying them buys nothing.
    - History: the same copy backed `list_from_section` at the baseline and goes back at least to 2026-09-30. `475fc39cb` added `markdown` to every `Section`, so each copy grew. The prompt reads debt removal plan deferred it once.
    - Impact: the copying grows with sections entered times document size. It has no measured cost on today's prompts of a few KB, but it is pure waste, it grows with prompt size and fanout width, and it contradicts the shared, read-only parsed prompt the run already holds as `Arc<Prompt>`.
    - Reversal cost: low. `Section` and `Block` are internal to the Engine crates. The facade doesn't expose them, and no crate outside `crates/runvil-internal` reads them.
    - Target state: the goals above.
- Exposed pre-existing debt: none beyond D1-17.
- Rejected candidates: 20.
  - 14 residual-but-acceptable: intended contracts already bounded by `#[non_exhaustive]`, the compiler, or an existing test. They cover string-keyed install, the three new `InstallError` variants, Workshop's log-and-continue, registry ownership, the call-site fan-out, the silent native skip in `Default`, the reserved MCP server name, the `Package::description` surface, the `prompt` namespace and nil on a miss, string heading addresses, the test stub fan-out, the ambiguity branch's coverage, the `vibe/` status flip, and month-filed plans.
  - 2 weak or speculative: exact-equality reservation versus normalized twins (no present name collides), and doc drift across corrective commits as a recurring cause.
  - 4 false: the parser helpers as hard-to-reverse, `build_sections` as oversized (37 lines at the endpoint), `prompt` as an unprotected global, and `Native::call` as removable dead code.

</product-contract>
<implementation-contract>

## Technical Design

### Doc and message fixes (D1-1, CH-1, D1-10)

- No interface change. Every change here is a string literal or doc text.
- `crates/harness-internal/runner/src/packages.rs`:
  - `NATIVE`'s `.description(...)` (line 118) states present behavior. Suggested: "Reserves the subagents name for the Engine's subagent tools, which are not built yet".
  - `PluginKind::Native`'s doc (line 37). Suggested: "Built into every registry; its behavior will live in the Engine, and today's one native, `runvil/subagents`, has none yet."
  - Leave the constraint comment on `NATIVE` unchanged. It states rules for future code, not present behavior.
- `AGENTS.md` line 12: replace only the Engine-native clause. Suggested: "or Engine-native (`runvil/subagents`, reserved for subagent tools the Engine will implement under the fixed name `subagents`, which no other Plugin may take; not built yet)". Keep every other sentence word for word. The wording must pass the docs-claims scan: Plugin capitalized, and never "activated".
- The three `PACKAGE` doc lines become "The Plugin's label, which a Host adds to its package registry." Plugin crates don't depend on `harness`, so the doc uses no intra-doc link to `PackageRegistry`.
- `InstallError::UnknownPackage`'s `#[error(...)]` text in `crates/harness-internal/runner/src/host.rs` (line 246) becomes "Plugin package `{package}` is not in the registry; add it with PackageRegistry::add before building the HostContext". The variant, its field, and its doc comment stay as they are, apart from any doc sentence that repeats the old advice.

### Read visible sets in place (D1-17, Engine)

A section's visible set is the sections it may address by heading: its home slice (its siblings) minus itself, plus its own direct children. Today that set is built as an owned copy. It becomes a borrowed view over the shared prompt instead.

- `crates/runvil-internal/engine/src/heading_address.rs` (116 lines) gains the view, beside the matching code that reads it:

```rust
/// A section's visible set, read in place: its home slice minus itself,
/// plus its direct children.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Visible<'a> {
    home: &'a [Section],
    caller: Option<usize>,
    children: &'a [Section],
}

impl<'a> Visible<'a> {
    /// A walked section's set: `home` minus `home[caller]`, plus `home[caller]`'s children.
    pub(crate) fn of(home: &'a [Section], caller: usize) -> Visible<'a>;
    /// A whole slice and nothing else: the H1 pass's top-level set, and synthetic sets in tests.
    pub(crate) fn all(slice: &'a [Section]) -> Visible<'a>;
    /// The sections in today's order: the home slice in order with the caller skipped, then the children.
    pub(crate) fn iter(self) -> impl Iterator<Item = &'a Section>;
}
```

- `find_visible` (lines 73-90) and `resolve_visible` (lines 100-112) take `Visible<'a>` in place of `&'a [Section]`, with the same matching, ambiguity detection, and error text. `find_visible` walks `visible.iter()` once for the match and the ambiguity check. `resolve_visible`'s not-found message calls `visible.iter()` again to list the visible sections, which works because `Visible` is `Copy`, in the same order the copied `Vec` had.
- Update the module doc and function docs that describe the visible set as a slice, so they describe the view.
- `crates/runvil-internal/engine/src/execute/walk_target.rs` (108 lines):
  - Delete `home_without` and `visible_sections`.
  - Move `SlicePath` here from `scheduler.rs` (lines 103-144: the struct, its derives, its doc comment, and its four methods `root`, `child`, `resolve`, and `name_at`). Make the struct and all four methods `pub(super)`; today the methods are private to the scheduler module. Then the scheduler's files and `SectionContext` construction both reach it, because code under `execute/` sees a `pub(super)` item of `execute::walk_target`. The scheduler's files import it from `walk_target` in place of their current `super::SlicePath` paths.
  - `read_visible(heading: &str, visible: Visible<'_>) -> Result<Option<SectionText>>`.
  - `resolve_jump_target(heading: &str, siblings: &[Section], jumper: usize) -> Result<JumpTarget>`: it resolves against `Visible::of(siblings, jumper)` and maps the match to `JumpTarget::Child` or `JumpTarget::Sibling` as today, keeping its internal-invariant error.
  - `h1_visible_sections` and `section_position` stay. Both already borrow.
- `crates/runvil-internal/engine/src/execute/section_context-construct.rs` (194 lines):
  - `SectionContext::new` takes `home: &SlicePath` and `index: usize` in place of `section: &Section` and `siblings: &[Section]`, so its argument count is unchanged. It finds its section with `home.resolve(ctx.prompt()).get(index)`, and a miss returns `Error::internal("an entered section is absent from its slice")`; the caller resolved the same slice and index just before, so a miss is an internal invariant violation. Update the `#[expect(clippy::too_many_arguments, reason = ...)]` text to name the slice path and index, and the doc comment (lines 35-37) that says the visible set "is built" from `siblings`.
  - Its read callback captures `Arc::clone(ctx.prompt_arc())`, a clone of the `SlicePath`, and `index`. On each call it resolves the home slice from the shared prompt and reads through `Visible::of`. The callback bound, `Fn(String) -> Result<Option<SectionText>, Error> + Send + Sync + 'static` (`section_vm.rs` lines 116-122), is unchanged. `Arc<Prompt>`, `SlicePath`, and `usize` are all `Send + Sync + 'static`.
  - `new_live_h1`'s callback captures `Arc::clone(ctx.prompt_arc())` and reads through `Visible::all(h1_visible_sections(&prompt))`, replacing the `.to_vec()` at line 168.
- `crates/runvil-internal/engine/src/execute/scheduler/walk.rs`: `enter_section` (around line 160) passes `&chain.slice` and `chain.index` to `SectionContext::new`. `resolve_chain_target` (line 303) passes `index` to `resolve_jump_target`, and its H1 branch (line 290) resolves against `Visible::all(sections)`. `scheduler/h1.rs` line 148 does the same.
- `SectionText` (`crates/runvil-internal/lua/src/handles.rs` lines 144-149) stays an owned value. It is the one copy left, made only when a read is called, of the one section asked for, on its way into Lua's own strings.

### One shared body string (D1-17, parser)

- New module `crates/runvil-internal/parser/src/body_text.rs`, declared in `lib.rs`, with its tests in a sibling `body_text-tests.rs`. `BodyText` is re-exported beside `Section` and `Block`:

```rust
/// A stretch of a prompt's text, read in place from the one shared copy.
#[derive(Clone)]
pub struct BodyText {
    text: Arc<str>,
    range: Range<usize>,
}

impl BodyText {
    /// The text this stretch covers.
    pub fn as_str(&self) -> &str;
    /// `part` located inside `text` by its byte offset; `None` when `part` is not a subslice of `text`.
    pub(crate) fn within(text: &Arc<str>, part: &str) -> Option<BodyText>;
    /// A whole owned string, for synthetic sections in test support.
    pub(crate) fn from_owned(text: String) -> BodyText;
}
```

  - `PartialEq` and `Eq` compare the text read, not the pointer or the range, so the derived equality on `Section`, `Block`, and `Prompt` still compares content.
  - `Debug` prints exactly what the `String` field printed (delegate to `<str as Debug>::fmt` on `as_str()`), so any debug output or `{:?}` text built from sections and blocks is unchanged.
  - Add `AsRef<str>`, `PartialEq<str>`, and `PartialEq<&str>`, so tests can compare a `BodyText`, or a slice of them, against string literals.
  - `within` finds `part`'s byte offset from the two strings' addresses (`str::as_ptr` with `.addr()`, stable since Rust 1.84; the toolchain is stable 1.99 and the workspace sets no minimum version). It uses no `unsafe`, and checks that `part` lies wholly inside `text`.
- The body: `split_frontmatter` already rebuilds the body as one LF-normalized `String`, and every later offset, span, and line number already counts from it. `parse_inner` (`parse.rs` line 64) turns it into one `Arc<str>` right after the split, and parsing reads that. So spans, line numbers, and parse errors don't change.
- `parse_body` and the section builders receive the shared `&Arc<str>` beside the `&str` they read today, so each builder can make a `BodyText`.
- `Heading.content` (`build.rs` lines 136-146, today `body[start..end].to_string()`) becomes a borrowed `&str` slice of the shared body, so `Heading` gains a lifetime.
- The intermediate values stop copying too:
  - `RawBlock::Prose` (from `split_section_blocks` in `fence.rs`) carries a `&str` in place of a `String`. Drop the `.to_string()` after each `pending_prose(...).trim()` at `fence.rs` lines 259, 292, and 305.
  - `parse_bullet_items` in `list.rs` yields `&str` items in place of `content.to_string()` (lines 23-29 and 93-136).
  - The markdown trim at `build.rs` line 317 keeps the `&str` instead of calling `.to_owned()`.
- Every one of those `&str`s is then a subslice of the shared body. Each becomes a `BodyText` through `BodyText::within`, where `Section` and `Block` are built (`build.rs` lines 171 and 311-318). A `None` there is a parser bug, and it returns `Error::Internal("parser: section text is not a slice of the prompt body")` (the variant at `error.rs` line 68), never a panic, because `expect` is denied.
- Lua fence sources keep going to `LuaProgram::compile(&source, ...)`, which copies them as it does today. Passing a `&str` there in place of a freshly built `String` is fine either way.
- `Section` fields (`prompt.rs` lines 31-45):
  - `markdown: BodyText`. `markdown()` still returns `&str`.
  - `items: Vec<BodyText>`. `items()` returns `&[BodyText]`, and readers call `as_str()`.
  - `name: String`, unchanged: names are decoded text, not stretches of the source (user decision).
  - `blocks` and `children` keep their shape.
- `Block::Prose { text: BodyText }`. `Section::prose()` still returns `&str`. `Block::Lua(LuaProgram)` is unchanged.
- H1: `split_h1` always splits a copy of the H1 content, not the body (`fence.rs` lines 53-84). It copies the content into `h1_content` at line 53, blanks the shared Lua fence out of that copy when one exists (line 63), and splits the copy at line 84. So H1 prose can hold blanked stretches the body doesn't. Turn that copy into its own `Arc<str>` before the split (once per prompt), and make H1 `Block::Prose` text through `BodyText::within` against it (line 88). The empty-prose check (lines 104-109) and `description_text` (lines 110-117) read `as_str()`. `title` and `description_text` stay owned `String`s, since they are decoded or joined text built once per prompt.
- Test support: `synthetic_section` and `prose_block` (`test_support.rs` lines 14-29) build their text with `BodyText::from_owned`, so Engine tests keep calling them the same way.
- Engine readers:
  - `read_visible` builds `SectionText` from `markdown().to_owned()` and each item's `as_str().to_owned()`.
  - `scheduler/step.rs` lines 85-98: `pending_prose` (`chain_record.rs` line 185, today `Option<String>`) becomes `Option<BodyText>` and takes a clone, which increments a shared count instead of copying the prose. The chain record is never saved or serialized (it derives no `Serialize`), so this changes no stored format. `start_lua` (line 150) passes `as_str()` to `install_lazy_prose`. `chain.rs` line 102 and `walk.rs` line 136 already set it to `None` and need no change.
  - `tasks.rs` lines 199-212 read `items()` for the list-worker checks. Adapt them to `&[BodyText]`.
- Size ceiling: every Rust file stays under 500 lines (`build_ceiling::check()`). Today `build.rs` has 325, `fence.rs` 327, `prompt.rs` 200, `scheduler.rs` 364, and `walk.rs` 316. `BodyText` gets its own module, and moving `SlicePath` shrinks `scheduler.rs`.

</implementation-contract>
<verification-contract>

## Testing Plan

- Doc and message fixes: no new tests. The facts the new text states are already pinned:
  - `natives_offer_no_tools_prelude_or_needs` (`crates/harness-internal/runner/src/packages-tests.rs`) and `an_installed_native_adds_no_catalog_tool_and_no_prelude` (`crates/harness-internal/runner/src/host-tests.rs`) pin that the native offers nothing (D1-1).
  - `a_new_registry_holds_the_native_and_lists_every_package_in_name_order` (`crates/harness-internal/runner/tests/it/packages.rs`) asserts the native's description is non-empty, and must still pass (D1-1).
  - The compiler's check on `install`'s `&str` parameter already stops the misuse the old docs described (CH-1).
  - `install_refuses_a_package_the_registry_does_not_hold` (`crates/harness-internal/runner/tests/it/packages.rs`) asserts the message contains the package name and `PackageRegistry::add`, so it keeps passing with the new wording (D1-10).
- D1-17 changes no behavior, so the existing tests are its regression suite. Each must still assert the same facts. Only call sites and the expressions that build expected values may change shape, for example `Block::Prose { text: BodyText::from_owned(...) }` in parser tests, which can call the crate-private constructor:
  - Reads: `prompt_reads_resolve_over_the_visible_set` and `prompt_reads_raise_only_for_mistakes_in_the_call` (`crates/runvil-internal/engine/src/execute/tests/suite/exec_flow/prompt_reads.rs`), and the fanout collection tests in `exec_flow/fanout_collections.rs` and `exec_flow/fanout_arms.rs`.
  - Addressing and ambiguity: `heading_address-tests.rs`, including `resolve_visible_rejects_more_than_one_match`, which moves to `Visible::all`, and `duplicate_top_level_section_names_error_loudly` (`execute/tests/exec_flow.rs`), which passes the jumper's index.
  - Jumps, calls, and fanout targets: `execute/tests/scheduler/walk/jumps.rs`, `execute/tests/scheduler/fanout/arms.rs` (including `a_jump_inside_a_fanout_arm_drives_a_child_walk` and `a_jump_from_an_arm_to_a_worker_child_walks_the_child_slice`), and `tasks-chain-end.rs`.
  - Parsing: every parser test of markdown, items, prose, thematic breaks, fences, and line mapping, including the CRLF cases in `parser/src/tests/h1_fences.rs` and `section_fences.rs` and the H1 prose around a shared fence in `h1_fences.rs`.
- New unit tests in `crates/runvil-internal/parser/src/body_text-tests.rs`:
  - `a_body_text_reads_back_the_part_it_was_made_from`: `within` on a subslice returns a `BodyText` whose `as_str()` is that subslice, and which equals a plain `&str` with the same text.
  - `a_string_outside_the_shared_text_is_refused`: `within` on an unrelated `String` with the same characters returns `None`.
  - `equal_text_from_different_stretches_compares_equal`: two `BodyText`s over different ranges, or different shared strings, holding the same characters are equal, so derived equality on sections compares content.
- Compiler checks carry the structural goals, so no shape test is added: `Section` and `Block::Prose` hold `BodyText` fields, and `home_without` and `visible_sections` no longer exist to call.
- Exit checks:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - both clippy runs from the Project Survey, with `CARGO_BUILD_WARNINGS=deny`
  - `cargo fmt --all --check`
  - `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` and `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`, both with `RUSTDOCFLAGS="-D warnings"`
  - `cargo +nightly-2026-09-05 xtask api --check` passes unchanged, since the facade exposes neither `Section` nor `Block`
  - `cargo test -p build-xtask` (crate boundaries and the 500-line ceiling)
  - `node --test crates/workshop/ui/test/docs-claims.mjs` (D1-1's `AGENTS.md` wording)
  - `rg -n "Lets the model start|passes to its install|implemented in the Engine under the fixed name|before installing it" crates AGENTS.md` prints nothing (D1-1, CH-1, D1-10)
  - `rg -n "fn visible_sections|fn home_without|Vec<Section>" crates/runvil-internal/engine/src --glob '!**/*tests*' --glob '!**/tests/**'` prints nothing (D1-17). Today it prints exactly `walk_target.rs` lines 28 and 39.

</verification-contract>
<decision-record>

## Decision Record

- User-resolved choices:
  - One shared body string, with sections recording offsets into it to form a tree that mirrors the Markdown structure. User: "I like the idea of having one big shared string, and then computing offsets into it, to constructed a hierarchal table of sections, mirroring the markdown structure."
  - Read the shared prompt in place, with no copies. User: "Why are we borrowing at all? prompt text is read-only. And when someone asks for an H2, they should not get the child sections." Reads already return only the section's own text, so the copies are pure waste.
  - Keep heading names as owned, decoded strings. `## Q&amp;A` is addressed as `Q&A`, exactly as today. The user chose this over raw-text names, which would change how authors address headings.
  - Fold D1-17 into this plan rather than giving it a plan of its own, so the doc fixes and the redesign land in one run. The user chose this knowing the doc fixes then wait for the redesign to be built and verified.
  - Fix D1-10 now, though both analysis passes rejected it as wording-only. The user agreed when shown that the advice names a step a Host can no longer take.
  - Leave the task built-ins ungated until the `tasks` and `subagents` overhaul. User: "We are going to overhaul `tasks` and `subagents` later anyway. so the tools.allow_tasks problem is going to just vanish" and "Fix it later." The reasoning: `tasks` is a language feature, pure Lua, and a prompt that doesn't want it just doesn't use it. `subagents` is model-facing, so it must behave like a linked Plugin, with the Host's install deciding whether runs get its tools. That makes D1-1's present-tense text wrong today and right only after the overhaul.
- Reversible decisions and consequences:
  - Each text field holds a `BodyText`: a handle to the shared string plus a byte range. Consequence: one shared pointer per text field. In exchange, `markdown()` and `prose()` keep their signatures, and synthetic test sections stay easy to build.
  - `SectionText` stays owned. That is the only copy left: made when a read is called, of the requested section only, on its way into Lua.
  - `LuaProgram` is unchanged. It lives in `runvil-lua`, which does not depend on the parser. It compiles to bytecode once per parse, and only tests and diagnostics read its source.
  - H1 prose points into its own masked copy of the H1 content, because the parser blanks the shared fence out of that copy before splitting it.
  - `SlicePath` moves to `walk_target.rs`, beside the visible-set code, so the scheduler and frame construction share it.
  - Rewrite the native's text in the present tense now, rather than leaving the planned wording until the overhaul. Consequence: the overhaul rewrites these three lines again when the subagent tools land. Accepted, because `list()` is public and the deferred checkbox screen would show the false line.
  - No test pins the exact description or doc wording. Pinning text would be a source-shape ratchet with no product contract behind it.
  - One doc pass covers D1-1 and CH-1, since they share a cause.
  - The `runvil/tasks` case of D1-10 keeps the same message. It is still true for that name, just unhelpful.
  - `SectionContext::new` resolves its section by slice path and index, rather than taking both the section and the path. Consequence: one lookup it already had in hand is repeated, in exchange for an argument list that stays the same length and a single source for which section is entered.
  - The section entry's lookup miss is an internal error, not a panic, matching `resolve_jump_target`'s existing internal-invariant error.
- No remediation changes a public interface, persisted or wire format, component ownership, dependency direction, or trust boundary.
- Rejected alternatives:
  - Bare byte ranges, with every text accessor taking the prompt or body as an argument. Rejected: every reader and many parser tests change signature, for no gain over a shared handle.
  - Heading names as raw-text offsets. Rejected by the user: authors would have to write `jump('## Q&amp;A')`.
  - Only one half of D1-17. The shared string alone still builds and discards a list of sections on every section start. In-place reads alone leave the parsed tree storing some text twice.
  - `Arc<Section>` children, so clones become cheap. Rejected: lists would still be built and thrown away, and reading in place removes them.
  - Writing read results straight into Lua strings. Rejected for now: it changes the `runvil-lua` callback interface, to save one copy per read call.
  - A test asserting that section text points into the shared body. Rejected: it checks implementation shape, and the types already guarantee no owned text.
  - Keep the forward-looking native description and record a `Deferred:` note. Rejected: the field's documented meaning is what the Plugin offers, and a note in history does not reach a Host reading `list()`.
  - Gate the task built-ins on the install now. Rejected: that is the overhaul's coordinated Engine and Harness change, and the user chose to fix it later.
  - Give the native an empty description. Rejected: the integration test asserts a non-empty one so a listing can say what the package is.
- Assumptions and risks:
  - `BodyText::within` relies on every derived `&str` being a subslice of the shared string. Making `Heading.content` a borrowed slice establishes that, and a stray copy fails as an internal parse error instead of quietly pointing at the wrong text.
  - An error that lists the visible set must keep today's order. `Visible::iter` yields the home slice in order with the caller skipped, then the children, which is the order the copied `Vec` had.
  - `Debug` output for sections and blocks must not change. `BodyText`'s `Debug` delegates to the text's own, so a `{:?}` of a section prints what it printed before.
  - Moving `SlicePath` touches every scheduler file that names it. The compiler finds each one, and each change is an import path.
  - The suggested doc wording may need adjusting to pass the docs-claims scan. The scan decides.
  - The analysis did not inspect repositories outside `runvil`. A follow-up search found no stale install wording in `runvil-docs`, and only historical hits (old chats, scratch, earlier drafts) in `runvil-design`.

### Deferred and Out of Scope

- Gating the four task built-ins on the native's install. Revisit in the `tasks` and `subagents` overhaul, which also rewrites the native's description, the `AGENTS.md` clause, and the `PluginKind::Native` doc to describe real behavior.
- `LuaProgram` keeping its own copy of each fence's source. Revisit if Lua source memory matters, or when `LuaProgram` is next reworked.
- Writing read results directly into Lua strings, which would remove the last copy per read. Revisit if reads show up in profiles.
- Exporting the native's package name as a constant from `harness::plugin` (D1-2). Revisit when a second Host installs the native by name.
- Normalized comparison in `PackageRegistry::reserves` (D1-7). Revisit when a second native, or a native name containing `-`, `_`, or `.`, is added.
- The user guide in `runvil-docs` still teaches the removed `list_from_section` global: 35 mentions across nine `src/language/` chapters (most in `14-fanout.md`, `08-jump-and-call.md`, and `03-blocks-and-prose.md`), and no mention of `prompt.section` or `prompt.list`. Its last commit (`44ff253`, 2026-10-09) predates the removal in `7a80775b7`. It is a separate repository, so it gets its own change, not a work item here.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway`.
  - `crates/gateway/app` is the workspace's only default member, so plain `cargo build` builds the same thing. The Engine and Plugin crates compile under any test, clippy, or docs command that names them.
  - The desktop app builds with `cargo workshop`, an alias for `run -p build-workshop --` in `.cargo/config.toml`.
  - The `gateway-config-ui` and `workshop-server` build scripts bundle their UIs with esbuild, so workspace-wide commands need Node.js plus one `npm ci --prefix crates/workshop` and one `npm ci --prefix crates/gateway/config-ui/ui` per checkout. Both installs are present on this machine, along with Node.js 24 (CI uses 22), cargo-nextest 0.9.128, stable Rust, and `nightly-2026-09-05`.
  - Workshop clippy and tests need the gateway sidecar staged for Tauri's `externalBin`: `cargo build --locked -p gateway --no-default-features`, then `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe`, and `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc` afterward. A sidecar from an earlier build today is staged in `crates/workshop/desktop/binaries/` now.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-filter>`, such as `-p runvil-parser` or `-p runvil-engine`. Drop `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`, which CI never builds with all features.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`, with the same three exceptions. `cargo test -p build-xtask` runs the boundary and structural checks, and `cargo xtask tidy` prints the same report.
- Full-suite test command:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. The first run includes `build-xtask`.
  - CI also runs `cargo nextest run --locked -p workshop-workspace --all-features` and `cargo nextest run --locked -p workshop-server --features headless`.
  - The wording and rulebook scan: `node --test crates/workshop/ui/test/docs-claims.mjs`. It needs no `npm ci` and runs from any directory.
  - The UI suites: `npm test --workspaces --if-present` in `crates/workshop` and `npm test` in `crates/gateway/config-ui/ui`.
- Linter command:
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, both with `CARGO_BUILD_WARNINGS=deny` (in PowerShell, `$env:CARGO_BUILD_WARNINGS='deny'`).
  - Never run a standalone `cargo check --workspace` beside them. The one exception is `cargo check -p gateway --no-default-features`, the headless build shape clippy does not cover.
  - UI type checks: `npm run typecheck --workspaces --if-present` in `crates/workshop` and `npm run typecheck` in `crates/gateway/config-ui/ui`.
  - Supply chain, run in CI: `cargo deny check`, `cargo audit`, `cargo hakari verify`, and a `cargo tree` check that `ring` stays out of the gateway's normal closure.
- Formatter check command: `cargo fmt --all --check`, with `style_edition = "2024"` in `rustfmt.toml`. The pre-commit hook runs it.
- Docs command:
  - Each with `RUSTDOCFLAGS="-D warnings"` (in PowerShell, `$env:RUSTDOCFLAGS='-D warnings'`): `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, `cargo doc -p runvil --no-deps`, `cargo doc -p harness --no-deps`, `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`, and `cargo doc --locked --no-deps -p workshop-server --document-private-items`. The facade docs build with default features on purpose.
  - The facade surface check: `cargo +nightly-2026-09-05 xtask api --check` against `crates/runvil/public-api.txt`; `--bless` regenerates the listing. The nightly is pinned in `crates/build-xtask/src/api/toolchain.rs`, and its fixtures run with `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`.
  - The documentation site: `cargo xtask site [--books-only]`, which needs `RUNVIL_DOCS` naming a `runvil-docs` checkout and an mdBook binary (`MDBOOK`, default `mdbook`).
- Test placement and naming conventions:
  - Unit tests mostly sit in a sibling file wired with `#[cfg(test)] #[path = "<stem>-tests.rs"] mod tests;`, such as `crates/runvil-internal/parser/src/build-tests.rs`. A module whose tests reach three files keeps them in a `tests/` subdirectory beside a `tests.rs` that declares them, one file per topic, as in `crates/runvil-internal/parser/src/tests/` and `crates/runvil-internal/engine/src/execute/tests/`. Inline `#[cfg(test)] mod tests { }` blocks are also common, in about 150 files across most families, including `parser/src/list.rs` and `engine/src/execute/bindings.rs`.
  - Integration tests compile as one binary per crate: from `tests/it/main.rs` in most crates, and from `tests/suite/main.rs` in `harness` and `runvil`. Each topic is one module file, and shared helpers go in `support.rs`. `crates/runvil/tests/prompts/` holds prompt fixtures.
  - A few crates keep standalone test files directly in `tests/`: `gateway-stt-engine`, `gateway-stt-backend-whisper`, `gateway-cloud-providers`, and `build-workshop`. `runvil-engine` has one bench, `benches/models_loop.rs`.
  - Test names are snake_case sentences stating the behavior, such as `the_workspace_has_no_doctests`.
  - Doctests are banned. `build-xtask`'s `no_doctests` check fails any doc comment holding a code block rustdoc would compile, so examples use `text`, `json`, or `toml` fences.
  - Nightly-only `build-xtask` fixtures and the native whisper tests are `#[ignore]`d. `.config/nextest.toml` puts the STT crates in a `heavy` test group.
- Directory map:
  - `crates/`: all 52 Cargo packages. The top level holds the public and shared layer: the `runvil` facade, `runvil-plugin`, `harness`, `harness-gateway-client`, `gateway-api-types`, `gateway-api-discovery`, the `plugin-*` crates (`plugin-web`, `plugin-user-input`, `plugin-mcp`), `shared-error-source`, `shared-loopback`, the `build-*` tooling (`build-ceiling`, `build-llama-cuda`, `build-ui`, `build-user-guide`, `build-workshop`, `build-xtask`), and `workspace-hack`.
  - `crates/runvil-internal/`: the Engine's private crates `types`, `engine`, `lua`, `parser`, `vfs`, and `model-client`.
  - `crates/harness-internal/runner`: the Harness's private per-run runner and effect loop, including the package registry and `HostContext`.
  - `crates/gateway/`: the gateway's private crates `app`, `cloud-providers`, `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, and `web-search`, plus the `stt/` subsystem (`api`, `engine`, `backend-whisper`, `whisper-ffi`).
  - `crates/workshop/`: Workshop's crates `desktop` (package `workshop`), `server`, `server-api`, `agents`, `gateway`, `menu`, `protocol`, `registry`, `run-log`, `status`, `support`, `user-state`, and `workspace`, plus the npm workspace packages `ui`, `look`, and `platform`.
  - `crates/shared-ui`: a TypeScript and CSS package the gateway config UI consumes, not a Rust crate.
  - `guide/`: mdBook files for the gateway, language, and Workshop books, the shared page chrome, and the landing page style. The chapters live in the separate `runvil-docs` checkout.
  - `prompts/`: five example prompt files. `images/`: README art.
  - `tools/`: Node scripts for staging the gateway sidecar and a live TTS check, each with a `.test.mjs`.
  - `vibe/`: plan records, recent ones at the top level and older ones filed by month, plus an ignored `scratch/`.
  - `.github/workflows/`: `ci.yml`, whose `ci-green` job is the single gate, plus release, nightly, site, and native-library workflows.
  - `.config/`: `nextest.toml` and `hakari.toml`. `.cargo/config.toml`: the `xtask` and `workshop` aliases and the Windows `rust-lld` linker with the static CRT. `.githooks/`: pre-commit (format) and pre-push (wording scan, headless check, clippy, deny), with `core.hooksPath` set to it. `.cursor/rules/`: the Workshop architecture and SPA rules.
  - Not tracked: `local/`, `target/`, and `target-msrv/` (ignored), and `cabinet/`, which holds only an empty `_trash/`.
- Component boundaries, which `cargo test -p build-xtask` enforces across normal, dev, and build dependencies:
  - The Engine (`runvil*`) depends on no gateway, Workshop, Harness, or Plugin crate. Outside crates reach it only through `runvil` and `runvil-plugin`, the only two crates that may enter `runvil-internal/`.
  - Inside the Engine: `types` and `vfs` depend on no Engine crate, `model-client` on `types`, `lua` on `types`, `vfs`, and `model-client`, `parser` on `types` and `lua`, and `engine` on all five. The facade re-exports all six, and `runvil-plugin` names only `types` and `vfs`. `crates/runvil/public-api.txt` pins the facade's surface.
  - The Harness (`harness*`) names only `runvil`, `runvil-plugin`, and `workspace-hack` outside its family. The exception is `harness-gateway-client`, which depends on `plugin-web` to implement its `SearchProvider`. Outside crates reach the Harness only through `harness` and `harness-gateway-client`, and only `harness` may enter `harness-internal/`. The chain runs `harness-gateway-client` on `harness`, `harness` on `harness-runner`, and `harness-runner` on `runvil` and `runvil-plugin`.
  - Plugin crates depend only on `runvil-plugin`, `shared-*` crates, `workspace-hack`, and outside libraries. Their only dependents are `harness-gateway-client`, `workshop-server`, and `workshop-agents`. The native `runvil/subagents` package is not a Plugin crate: its `NATIVE` entry and `Native` object, which offers no tools, are defined in `crates/harness-internal/runner/src/packages.rs`.
  - The gateway (`gateway*`) depends on no Engine, Workshop, Harness, or Plugin crate. Outside crates reach it only through `gateway-api-types` and `gateway-api-discovery`. Inside the family, only `gateway-stt` is visible outside `gateway/stt/`.
  - Workshop is a Host. It reaches the gateway only through that public pair, the Engine only through `runvil` and `runvil-plugin`, and the Harness only through `harness` and `harness-gateway-client`. Its tiers depend one way: the server on features (`agents`, `workspace`, `user-state`, `run-log`), features on services (`menu`, `gateway`, `status`), and services on vocabulary (`protocol`, `registry`, `support`). The desktop app names only `workshop-server-api`, which names only `workshop-server`.
  - `shared-*` crates depend on no product crate. `build-*` crates are tooling and may depend into any container.
- Conventions summary:
  - Engine, Harness, Host, and Plugin are capitalized with one meaning each. Engine crates say "the caller" and never mention the Host. `docs-claims.mjs` enforces what a scan can see in every `AGENTS.md`, every `## Invariants` crate doc, and every `.cursor/rules` file.
  - 25 crate `lib.rs` files open with a `//!` doc carrying a `## Invariants` section, including every Engine-internal crate, `runvil-plugin`, each `plugin-*` crate, `harness-runner`, and every Workshop library.
  - Every crate's build script calls `build_ceiling::check()`, which fails any Rust file over 500 lines. Splits go into private child modules without widening visibility.
  - Clippy `all` and `pedantic` deny, `unwrap_used` and `expect_used` deny outside tests, and `allow_attributes` deny, so suppressions use `#[expect(..., reason = "...")]`. `missing_docs`, `missing_debug_implementations`, and `unreachable_pub` warn, which is fatal under the gate. `unsafe_code` is denied outside its owned boundaries, rustdoc's broken and private intra-doc links deny, and `clippy.toml` bans process-global installers outside binary entry points.
  - Source directories stay flat until a group reaches three files. Below that, kebab siblings such as `foo-bar.rs` are wired with `#[path]`.
  - Comments explain only non-obvious constraints, and every workaround cites its upstream issue URL.
  - Error messages are written for a model to read: concise, naming what was required and what was found.
  - Behavior changes ship with tests in the same change. A Cargo feature gates only a real build constraint, never product shape.
  - JSON that reaches a recorder round-trips exactly, with sorted keys and finite numbers.
  - Workspace dependencies are declared once in the root `Cargo.toml`, with a comment on every non-obvious pin. Every member inherits `workspace-hack`, which `cargo hakari` manages.
  - Commits carry a one-line summary, a prose body, a bullet list of changes, optional `Design:` and `Deferred:` lines, and a `Plan: vibe/<file>` trailer. A plan's closing commit is titled `Close plan: <words>`.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Correct the native and Plugin package docs [completed]

- Component: Plugin package docs
- Component placement: first of three. It depends on nothing, touches only `harness-runner`, the three `plugin-*` crates, and `AGENTS.md`, and shares no file with the D1-17 work, so the false Host-facing text is corrected before the larger redesign starts.
- Piece: native and Plugin doc pass (D1-1, CH-1). Built sequentially before the refusal-advice piece. The two pieces are independent; the doc pass goes first because D1-1 and CH-1 share one cause (the incomplete doc sweep in `92bff15b4`) and one pass fixes both.
- Changes:
  - `crates/harness-internal/runner/src/packages.rs`: `NATIVE`'s `.description(...)` and the `PluginKind::Native` doc comment state present behavior, using the Technical Design's suggested wording. Leave the constraint comment on `NATIVE` unchanged.
  - `AGENTS.md` line 12: replace only the Engine-native clause with the Technical Design's suggested wording, keeping every other sentence word for word.
  - The `PACKAGE` doc line in `crates/plugin-web/src/web.rs`, `crates/plugin-user-input/src/lib.rs`, and `crates/plugin-mcp/src/lib.rs` becomes "The Plugin's label, which a Host adds to its package registry." No intra-doc link to `PackageRegistry`.
- Tests: none new. `natives_offer_no_tools_prelude_or_needs`, `an_installed_native_adds_no_catalog_tool_and_no_prelude`, and `a_new_registry_holds_the_native_and_lists_every_package_in_name_order` pass unchanged.
- Verify: `cargo nextest run --locked -p harness-runner -p plugin-web -p plugin-user-input -p plugin-mcp --all-features`; `node --test crates/workshop/ui/test/docs-claims.mjs` (if it rejects the suggested wording, adjust the wording; the scan decides); the first `rg` exit check prints only the `host.rs` "before installing it" line, which Step 2 removes.
- Commit: one commit with the five text edits, message per `commit.mdc`, with the `Plan: vibe/<file>` trailer.

</step-1>

<step-2>

### Step 2: Make the UnknownPackage advice followable [completed]

- Component: Plugin package docs
- Piece: install refusal advice (D1-10). Follows the doc pass and shares no file with it.
- Changes:
  - `crates/harness-internal/runner/src/host.rs`: `InstallError::UnknownPackage`'s `#[error(...)]` text becomes "Plugin package `{package}` is not in the registry; add it with PackageRegistry::add before building the HostContext". Keep the variant, its field, and its doc comment, apart from any doc sentence that repeats the old advice. Add no special case for `runvil/tasks`; it gets the same message.
- Tests: none new. `install_refuses_a_package_the_registry_does_not_hold` (`crates/harness-internal/runner/tests/it/packages.rs`) still finds the package name and `PackageRegistry::add` in the message.
- Verify: `cargo nextest run --locked -p harness-runner --all-features packages::`; the first `rg` exit check prints nothing.
- Commit: one commit, message per `commit.mdc`, with the `Plan: vibe/<file>` trailer.

</step-2>

<step-3>

### Step 3: Resolve jump, call, and fanout targets in place [completed]

- Component: In-place visible sets
- Component placement: second. It depends on nothing and uses only the existing `Section` API. It precedes the shared-body component because both edit `read_visible`, so the parser's field-type change lands on readers that already borrow.
- Piece: target lookups through a borrowed view. Built sequentially before the section-entry piece, because that piece's read callbacks go through `Visible::of` and the view-taking `read_visible`, which this piece adds.
- Changes:
  - `crates/runvil-internal/engine/src/heading_address.rs`: add `pub(crate) struct Visible<'a>` (`Clone`, `Copy`, `Debug`; fields `home`, `caller: Option<usize>`, `children`) with `Visible::of(home, caller)`, `Visible::all(slice)`, and `iter()`, which yields the home slice in order with the caller skipped, then the children. `find_visible` and `resolve_visible` take `Visible<'a>` in place of `&'a [Section]` with the same matching, ambiguity detection, and error text; `resolve_visible`'s not-found message lists `visible.iter()` again. Update the module and function docs to describe the view.
  - `crates/runvil-internal/engine/src/execute/walk_target.rs`: `read_visible(heading: &str, visible: Visible<'_>)`. `resolve_jump_target(heading: &str, siblings: &[Section], jumper: usize)` resolves against `Visible::of(siblings, jumper)`, maps the match to `JumpTarget::Child` or `JumpTarget::Sibling`, and keeps its internal-invariant error. `h1_visible_sections` and `section_position` stay. `home_without` and `visible_sections` stay for one more step, used only by frame construction.
  - `crates/runvil-internal/engine/src/execute/scheduler/walk.rs`: `resolve_chain_target` passes `index` to `resolve_jump_target`, and its H1 branch (line 290) resolves against `Visible::all(sections)`. `scheduler/h1.rs` line 148 does the same.
  - `crates/runvil-internal/engine/src/execute/section_context-construct.rs`: interim only. Both read callbacks keep their owned copy and call `read_visible(&heading, Visible::all(&visible))`; Step 4 replaces them.
  - Test call sites: `heading_address-tests.rs` passes `Visible::all(&sections)`, including `resolve_visible_rejects_more_than_one_match`, and `duplicate_top_level_section_names_error_loudly` in `execute/tests/exec_flow.rs` passes the jumper's index `0`.
- Tests: none new; existing tests assert the same facts. Covered by `heading_address-tests.rs`, `execute/tests/scheduler/walk/jumps.rs`, `execute/tests/scheduler/fanout/arms.rs` (including `a_jump_inside_a_fanout_arm_drives_a_child_walk` and `a_jump_from_an_arm_to_a_worker_child_walks_the_child_slice`), `execute/tests/tasks-chain-end.rs`, and `duplicate_top_level_section_names_error_loudly`.
- Verify: `cargo nextest run --locked -p runvil-engine --all-features`; `cargo clippy -p runvil-engine --all-targets --all-features` with `CARGO_BUILD_WARNINGS=deny`.
- Commit: one commit, message per `commit.mdc`, with the `Plan: vibe/<file>` trailer.

</step-3>

<step-4>

### Step 4: Read prompt sections in place at section entry [completed]

- Component: In-place visible sets
- Piece: section-entry reads in place. Follows the target-lookup piece, whose `Visible` and view-taking `read_visible` it uses.
- Changes:
  - Move `SlicePath` (the struct, its derives, its doc comment, and its methods `root`, `child`, `resolve`, and `name_at`) from `crates/runvil-internal/engine/src/execute/scheduler.rs` into `walk_target.rs`, making the struct and all four methods `pub(super)`. `scheduler/chain.rs`, `chain_record.rs`, `h1.rs`, `test_hooks.rs`, and `walk.rs` import it from `crate::execute::walk_target` in place of `super::SlicePath`, and the `SlicePath` intra-doc link in the `scheduler.rs` module doc still resolves.
  - `section_context-construct.rs`: `SectionContext::new` takes `home: &SlicePath` and `index: usize` in place of `section: &Section` and `siblings: &[Section]`, finds its section with `home.resolve(ctx.prompt()).get(index)`, and returns `Error::internal("an entered section is absent from its slice")` on a miss. Its read callback captures `Arc::clone(ctx.prompt_arc())`, a `SlicePath` clone, and `index`, and reads through `Visible::of` over the resolved home slice on each call. `new_live_h1`'s callback captures `Arc::clone(ctx.prompt_arc())` and reads through `Visible::all(h1_visible_sections(&prompt))`, replacing the `.to_vec()` at line 168. The callback bound in `section_vm.rs` is unchanged. Update the `#[expect(clippy::too_many_arguments, reason = ...)]` text to name the slice path and index, and the doc comment (lines 35-37) that says the visible set "is built" from `siblings`.
  - `scheduler/walk.rs`: `enter_section` passes `&chain.slice` and the entered index to `SectionContext::new`.
  - `walk_target.rs`: delete `home_without` and `visible_sections`.
- Tests: none new. Covered by `prompt_reads_resolve_over_the_visible_set` and `prompt_reads_raise_only_for_mistakes_in_the_call` (`execute/tests/suite/exec_flow/prompt_reads.rs`), the fanout collection tests in `exec_flow/fanout_collections.rs` and `exec_flow/fanout_arms.rs`, and the rest of the Engine suite.
- Verify: `cargo nextest run --locked -p runvil-engine --all-features`; the second `rg` exit check prints nothing; `cargo clippy -p runvil-engine --all-targets --all-features` with `CARGO_BUILD_WARNINGS=deny`; `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items` with `RUSTDOCFLAGS="-D warnings"`; `cargo test -p build-xtask` (500-line ceiling).
- Commit: one commit, message per `commit.mdc`, with the `Plan: vibe/<file>` trailer.

</step-4>

<step-5>

### Step 5: Borrow parser intermediates from the body [completed]

- Component: Shared prompt body
- Component placement: third and last. It depends on the in-place component because both edit `read_visible`, and its Engine reader changes land on the in-place code.
- Piece: borrowed parser intermediates. Built sequentially before the storage piece, because `BodyText::within` needs every intermediate `&str` to be a subslice of the shared body, and this piece makes them so.
- Changes:
  - `crates/runvil-internal/parser/src/build.rs`: `Heading` gains a lifetime, and `content` becomes a `&str` slice of the body in place of `body[start..end].to_string()` (lines 136-146). The markdown trim at line 317 keeps the `&str` instead of calling `.to_owned()`.
  - `fence.rs`: `RawBlock::Prose` carries a `&str` in place of a `String`; drop the `.to_string()` after each `pending_prose(...).trim()` at lines 259, 292, and 305. `split_h1` still splits its own `h1_content` copy, so its prose borrows from that copy.
  - `list.rs`: `parse_bullet_items` returns `&str` items borrowed from its input (lines 23-29 and 93-136).
  - Interim: `Section` and `Block::Prose` still own `String`s, so the places that build them (`build.rs` lines 171 and 311-318, `fence.rs` line 88) copy once with `to_owned()`; Step 6 replaces those copies with `BodyText`. Lua fence sources go to `LuaProgram::compile` as today.
  - Inline tests in `list.rs` change only where an expected value's type changed.
- Tests: none new. Every parser test of markdown, items, prose, thematic breaks, fences, and line mapping, including the CRLF cases in `parser/src/tests/h1_fences.rs` and `section_fences.rs`, asserts the same facts.
- Verify: `cargo nextest run --locked -p runvil-parser --all-features`; `cargo clippy -p runvil-parser --all-targets --all-features` with `CARGO_BUILD_WARNINGS=deny`.
- Commit: one commit, message per `commit.mdc`, with the `Plan: vibe/<file>` trailer.

</step-5>

<step-6>

### Step 6: Store each prompt's text once as BodyText [completed]

- Component: Shared prompt body
- Piece: shared-body storage and its readers. Follows the borrowed-intermediates piece. The parser field changes and the Engine reader changes share one step because changing the `Section` and `Block::Prose` field types breaks the Engine readers at compile time.
- Changes:
  - New `crates/runvil-internal/parser/src/body_text.rs`, declared in `lib.rs` and re-exported beside `Section` and `Block`: `pub struct BodyText { text: Arc<str>, range: Range<usize> }` deriving `Clone`, with `pub fn as_str`, `pub(crate) fn within(text: &Arc<str>, part: &str) -> Option<BodyText>` (offset from `str::as_ptr` with `.addr()`, no `unsafe`, `None` unless `part` lies wholly inside `text`), and `pub(crate) fn from_owned(text: String)`. Gate `from_owned` with `#[cfg(any(test, feature = "test-support"))]`, since its only callers are parser tests and the feature-gated `test_support` module, and an ungated crate-private function is dead code in builds without the feature. Implement `PartialEq` and `Eq` on the text read, `Debug` by delegating to `<str as Debug>::fmt` on `as_str()`, `AsRef<str>`, `PartialEq<str>`, and `PartialEq<&str>`.
  - `parse.rs`: `parse_inner` turns the LF-normalized body from `split_frontmatter` into one `Arc<str>` right after the split. `parse_body` and the section builders receive that `&Arc<str>` beside the `&str` they read.
  - `prompt.rs`: `Section.markdown` becomes `BodyText` (`markdown()` still returns `&str`) and `Section.items` becomes `Vec<BodyText>` (`items()` returns `&[BodyText]`); `name` stays `String`. `Block::Prose { text: BodyText }`, with `prose()` still returning `&str`; `Block::Lua` is unchanged.
  - `build.rs` (lines 171 and 311-318) and `fence.rs` replace Step 5's interim `to_owned()` with `BodyText::within`. A `None` returns `Error::Internal("parser: section text is not a slice of the prompt body")`, never a panic.
  - `fence.rs` `split_h1`: turn the masked `h1_content` copy into its own `Arc<str>` before the split, and build H1 prose through `within` against it (line 88). The empty-prose check (lines 104-109) and `description_text` (lines 110-117) read `as_str()`; `title` and `description_text` stay `String`s.
  - `test_support.rs`: `synthetic_section` and `prose_block` build their text with `BodyText::from_owned` and keep their signatures.
  - Engine readers: `read_visible` in `walk_target.rs` builds `SectionText` from `markdown().to_owned()` and each item's `as_str().to_owned()`. The `pending_prose` field in `scheduler/chain_record.rs` becomes `Option<BodyText>`; `scheduler/step.rs` stores a clone of the block's `BodyText`, and `start_lua` passes `as_str()` to `install_lazy_prose`. The `items()` check in `scheduler/tasks.rs` adapts to `&[BodyText]`.
  - Parser test expected values change shape only, for example `Block::Prose { text: BodyText::from_owned(...) }`.
- Tests: new `crates/runvil-internal/parser/src/body_text-tests.rs`, wired with `#[cfg(test)] #[path = "body_text-tests.rs"] mod tests;`, holding `a_body_text_reads_back_the_part_it_was_made_from`, `a_string_outside_the_shared_text_is_refused`, and `equal_text_from_different_stretches_compares_equal`. Existing parser and Engine tests, including the H1 prose around a shared fence in `h1_fences.rs`, assert the same facts.
- Verify: `cargo nextest run --locked -p runvil-parser -p runvil-engine -p runvil-lua --all-features`. Then, as the plan's last step, run every Testing Plan exit check: both full nextest runs, both clippy runs with `CARGO_BUILD_WARNINGS=deny` (staging the gateway sidecar for the Workshop run as the Project Survey describes), `cargo fmt --all --check`, both `cargo doc` runs with `RUSTDOCFLAGS="-D warnings"`, `cargo +nightly-2026-09-05 xtask api --check` passing with `crates/runvil/public-api.txt` unchanged, `cargo test -p build-xtask`, `node --test crates/workshop/ui/test/docs-claims.mjs`, and both `rg` checks printing nothing. Fix any failure before this commit, in the files that own it.
- Commit: one commit, message per `commit.mdc`, with the `Plan: vibe/<file>` trailer.

</step-6>

</execution-plan>
