---
name: prompt namespace reads
overview: Add a `prompt` Lua namespace with `prompt.section(heading)` and `prompt.list(heading)`, both returning nil when the heading names no visible section, and remove `list_from_section`. One function answers every heading lookup and one callback serves both reads. Production code grows only by the new read, about 40 lines, and the tests shrink by about as much. Nothing about task execution changes.
todos:
  - id: parser-markdown
    content: Add Section.markdown and its accessor; set it in build_sections from Heading.content with trim_leading_blank_lines (now pub(crate)) and trim_end; synthetic_section defaults it to empty; one parser test in tests/headings.rs (blank line above its ---)
    status: pending
  - id: scope-lookup
    content: Add find_visible in heading_address.rs as the one heading lookup (Ok(None) when the visible set has no such heading) and make resolve_sibling a thin adapter over it with unchanged messages; one unit test in heading_address-tests.rs; lands alone, after parser-markdown in the same checkout (or beside it in a separate worktree)
    status: pending
  - id: prompt-change
    content: "One atomic change in three parts. Table: SectionText, read_visible replacing list_items_from_visible (plus the stale home_without doc paragraph), install_prompt_table replacing install_list_global (Send + Sync read callback), prompt for list_from_section in RESERVED_NAMES, the read callback through setup_section_vm and both SectionContext constructors, the seven test stubs. Removal: every remaining list_from_section use (calls, test names, doc comments, the four fanout fixtures, the collection.rs error). Tests: prompt_reads.rs replacing list_from_section.rs, the two nil-rule edits, the three deletions"
    status: pending
  - id: docs
    content: "Update the language spec: index entries, the whole H1 availability sentence (call and jump work there too), the deferred markdown note, and the list_from_section API entry replaced by prompt.section and prompt.list entries with Implementation status lines"
    status: pending
  - id: verify
    content: nextest (both sets), clippy with warnings denied (both sets), fmt, docs and facade docs builds, xtask api --check, build-xtask tests, the docs-claims test, and a list_from_section search over crates/ and the spec that finds nothing
    status: pending
isProject: false
---

# prompt namespace: prompt.section and prompt.list replace list_from_section

<product-contract>

## Product Requirements

Prompt authors need to read other sections' raw text, and the one cross-section reader they have, `list_from_section`, is a bare global that raises on every miss. This plan adds a `prompt` Lua namespace with `prompt.section(heading)` and `prompt.list(heading)`, both returning `nil` when the heading names no visible section, and removes `list_from_section`. One function answers every heading lookup. Production code grows only by the new read, about 40 lines, and the tests shrink by about as much, ending with fewer tests than they start with. Nothing about task execution changes.

- Problem and users:
  - Prompt authors cannot read another section's text. `list_from_section` returns a list section's items and refuses every other kind of section.
  - `list_from_section` raises when the heading isn't found, so an author can't ask "does this section exist?" without `pcall`.
  - It is a bare global beside `call`, `jump`, and `prose`, with no namespace to grow into.
- Goals:
  - `prompt.section(heading)` returns a visible section's own raw Markdown, byte for byte, or `nil` when the heading names no visible section.
  - `prompt.list(heading)` is today's `list_from_section` under the new name and also returns `nil` on a miss, so a heading can serve as an index: with sections `### anthropic`, `### openai`, and `### gemini`, `prompt.list('### grok')` is `nil`.
  - `list_from_section` is removed, with no stub.
  - One function answers every heading lookup: given a heading and a visible set, the section, or "not in scope".
- Non-goals:
  - Anything about task execution: `tasks`, the `fanout` global's behavior, subagents, the scheduler, tool-call dispatch, and the Harness.
  - Backward compatibility for `list_from_section`.
  - Addressing grandchild sections in one call.
  - A `markdown` namespace for parsing arbitrary text.
- Success criteria:
  - `prompt.section` returns a section's own content byte for byte, including Lua fences, `---` breaks, and unrendered `{{ }}` placeholders, and `""` for an empty visible section.
  - Both reads return `nil`, without raising, for a heading that doesn't exist and for one outside the caller's visible set (a niece, a grandchild, or the caller itself).
  - Both reads work from the H1 pass and from a `lua shared` library at load time.
  - `call` and `jump` behave exactly as before, error messages included.
  - No source file mentions `list_from_section` outside history files, and no VM has that global.
  - The change deletes more test code than it adds, and every exit gate passes.
- Constraints:
  - Engine crates name the code that steps a run "the caller" and never mention a Host, Workshop, or lowercase "harness". The docs-claims test scans every line of `crates/promptforge` and `crates/promptforge-internal`, comments and strings included.
  - No source change under `crates/promptforge-internal/engine/src/execute/scheduler/`. Test files under `crates/promptforge-internal/engine/src/execute/tests/scheduler/` are a different directory and change only as the Testing Plan lists.
  - Every `.rs` file stays under 500 lines, and source directories stay flat.
  - Prompts are at language version `promptforge: 0`, so removed forms simply stop existing.
- Open questions:
  - None.

## Functional Specification

Authors read other sections through `prompt`. A read resolves its heading over the section's visible set, the same set `call` and `jump` resolve against. A heading with no match in that set reads as `nil`, and a mistake in the call itself raises.

- Actors and workflows:
  - A *section* is a Markdown heading (H2 to H6) and its content. A section's *visible set* is its siblings minus itself, plus its direct children. The *H1 pass* is the Lua under the prompt's H1 title, and its visible set is the whole top-level slice. The *`lua shared` library* is a fenced Lua block in the H1 that replays at the start of every section VM.
  - A prompt author reads another section:

    ```lua
    local rules = prompt.section('### Instructions')   -- raw Markdown; nil if no such visible section
    local extra = prompt.section('### Extras') or ''   -- an optional read needs no pcall
    local topics = prompt.list('### Topics')           -- was list_from_section
    local picks = prompt.list('### ' .. vendor)        -- headings as an index: nil for a vendor with no section
    ```
- Inputs and outputs:
  - `prompt.section(heading)` returns a string, or `nil`.
  - `prompt.list(heading)` returns a Lua array of strings (a list section's items, markers stripped), or `nil`.
  - `heading` is a string such as `'### Instructions'`: the level markers, whitespace, and the exact name.
- States and validation:
  - `prompt.section` span: from just after the heading line to the next heading of any level, so the heading line and nested subsections are excluded. Leading blank lines and trailing whitespace are trimmed; everything between, including the first content line's indentation, is byte-exact. An empty section returns `""`, never `nil`. Any kind of section is valid, list sections included.
  - A heading that doesn't exist and one outside the visible set get the same answer, `nil`, because the lookup searches only the visible set. That is also why a miss reveals nothing about the rest of the document.
  - `prompt` is a reserved global, and `list_from_section` no longer exists.
- Errors and recovery:
  - Both reads raise at the call site, catchable by `pcall`, for a malformed heading (no `#` markers, no name, or no whitespace after the markers) and for a non-string argument.
  - `prompt.list` also raises "section `<heading>` has no pre-parsed items" for a visible section that isn't a list, since naming a prose section is a mistake in the call.
  - A `nil` passed on as a `fanout` collection fails with the collection error, which now points at `prompt.list` and says it returns `nil` when no visible section has that heading.
- Security and privacy behavior:
  - The reads expose only text already in the prompt, and only for sections in the caller's visible set. `{{ }}` placeholders come back unrendered, so a read evaluates no template and reads no run data.
- Acceptance criteria:
  - Every behavior above has a test in the Testing Plan, and every exit gate passes.

</product-contract>
<implementation-contract>

## Technical Design

One lookup, `find_visible`, searches a visible set for a heading and answers with the section or `None`. Two thin adapters sit on it: `resolve_sibling`, which keeps today's not-found error for `call` and `jump`, and `read_visible`, which hands a prompt read the section's text. The Lua crate takes one `read` callback in place of today's `list_callback`, the callback behind `list_from_section`, and builds both `prompt` functions over it, so no signature gains a parameter. The parser keeps each section's raw Markdown, the one piece of data that doesn't exist today.

- Architecture:

  ```mermaid
  flowchart LR
    reads["prompt reads"] --> readVisible["read_visible"]
    callJump["call and jump"] --> resolveSibling["resolve_sibling"]
    readVisible -->|"miss is nil"| findVisible["find_visible"]
    resolveSibling -->|"miss is an error"| findVisible
    findVisible -->|"searches only"| visibleSet["visible set"]
  ```

  - The visible set is built where it is today: `visible_sections` for a walked section, and the top-level slice for H1. Neither changes.
  - "Out of scope" and "doesn't exist" are one answer by construction: the lookup only ever sees the visible set.
- Modules and interfaces:
  - Paths in the code comments below are relative to `crates/promptforge-internal/`.

  ```rust
  // parser/src/prompt.rs
  pub struct Section { /* existing fields */ pub(crate) markdown: String }
  impl Section {
      /// The section's own raw Markdown: from just after its heading line to the next
      /// heading of any level, leading blank lines and trailing whitespace trimmed.
      pub fn markdown(&self) -> &str;
  }

  // parser/src/fence.rs: reused, now crate-visible
  pub(crate) fn trim_leading_blank_lines(content: &str) -> &str;

  // engine/src/heading_address.rs
  /// The one heading lookup: the section in `visible` with the heading's exact level
  /// and name, or `None` when the visible set has none.
  /// # Errors: a malformed heading, or two visible sections with one address.
  pub(crate) fn find_visible<'a>(heading: &str, visible: &'a [Section]) -> Result<Option<&'a Section>>;
  /// `find_visible` where a miss is an error (`call`, `jump`); callers and messages unchanged.
  pub(crate) fn resolve_sibling<'a>(heading: &str, visible: &'a [Section]) -> Result<&'a Section>;

  // engine/src/execute/walk_target.rs: replaces list_items_from_visible
  pub(super) fn read_visible(heading: &str, visible: &[Section]) -> Result<Option<SectionText>>;

  // lua/src/handles.rs, re-exported at the crate root and in engine/src/lua.rs
  /// What a prompt read sees of one visible section.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct SectionText {
      /// The section's own raw Markdown.
      pub markdown: String,
      /// Its pre-parsed list items; empty for a section that isn't a list.
      pub items: Vec<String>,
  }

  // lua/src/vm/install.rs
  impl SectionVm {
      pub fn install_scheduler_control_globals<R, E>(&self, read: R) -> Result<()> // was list_callback
      where
          R: Fn(String) -> std::result::Result<Option<SectionText>, E> + Send + Sync + 'static,
          E: std::error::Error + Send + Sync + 'static;
      #[cfg(test)]
      pub(crate) fn install_control_globals<E, R>(&self, call_callback: E, read: R) -> Result<()>
      where
          E: Fn(Value, Option<String>, Json) -> std::result::Result<String, Error> + Send + 'static,
          R: Fn(String) -> std::result::Result<Option<SectionText>, Error> + Send + Sync + 'static;
      fn install_prompt_table<R, E>(&self, globals: &mlua::Table, read: R) -> Result<()>; // replaces install_list_global
  }

  // engine/src/execute/section_vm.rs
  pub(crate) fn setup_section_vm<R>(vm: &mut SectionVm, setup: &SectionVmSetup<'_>, read: R) -> Result<()>
  where
      R: Fn(String) -> std::result::Result<Option<SectionText>, Error> + Send + Sync + 'static;
  ```

  The three bodies that hold logic. `find_visible` is today's `resolve_sibling` body without its not-found branch, and the adapter takes that branch over with the same text:

  ```rust
  pub(crate) fn find_visible<'a>(heading: &str, visible: &'a [Section]) -> Result<Option<&'a Section>> {
      let (level, name) = parse_heading_address(heading)?;
      let mut matches = visible
          .iter()
          .filter(|section| usize::from(section.level()) == level && section.name() == name);
      let found = matches.next();
      if found.is_some() && matches.next().is_some() {
          return Err(Error::Lua(format!(
              "section heading `{}` is ambiguous; more than one visible section matches {} {name}",
              heading.trim(),
              "#".repeat(level)
          )));
      }
      Ok(found)
  }

  pub(crate) fn resolve_sibling<'a>(heading: &str, visible: &'a [Section]) -> Result<&'a Section> {
      find_visible(heading, visible)?.ok_or_else(|| {
          let available: Vec<String> = visible
              .iter()
              .map(|s| format!("{} {}", "#".repeat(s.level().into()), s.name()))
              .collect();
          Error::Lua(format!(
              "section heading `{}` not found; available sections: {}",
              heading.trim(),
              available.join(", ")
          ))
      })
  }

  pub(super) fn read_visible(heading: &str, visible: &[Section]) -> Result<Option<SectionText>> {
      Ok(heading_address::find_visible(heading, visible)?.map(|section| SectionText {
          markdown: section.markdown().to_owned(),
          items: section.items().to_vec(),
      }))
  }
  ```

  `install_prompt_table` builds both functions over one shared callback. The "no pre-parsed items" check moves here from the Engine, because it belongs to the `prompt.list` surface. Every `read` bound gains `Sync` (today's list callback bounds are `Send + 'static`): mlua's `send` feature makes each Lua function require `Send`, and `Arc<R>` is `Send` only when `R` is `Send + Sync`. Every current closure captures plain data (`Vec<Section>`, or nothing in the stubs), so none needs to change:

  ```rust
  fn install_prompt_table<R, E>(&self, globals: &mlua::Table, read: R) -> Result<()>
  where
      R: Fn(String) -> std::result::Result<Option<SectionText>, E> + Send + Sync + 'static,
      E: std::error::Error + Send + Sync + 'static,
  {
      let read = Arc::new(read);
      let read_markdown = Arc::clone(&read);
      let section = self
          .lua
          .create_function(move |_, target: Value| {
              let text = read_markdown(resolve_section_target(target)?).map_err(mlua::Error::external)?;
              Ok(text.map(|text| text.markdown))
          })
          .map_err(Error::lua)?;
      let list = self
          .lua
          .create_function(move |lua, target: Value| {
              let heading = resolve_section_target(target)?;
              match read(heading.clone()).map_err(mlua::Error::external)? {
                  None => Ok(Value::Nil),
                  Some(text) if text.items.is_empty() => Err(mlua::Error::external(format!(
                      "section `{}` has no pre-parsed items",
                      heading.trim()
                  ))),
                  Some(text) => pack_sequence(lua, text.items).map(Value::Table),
              }
          })
          .map_err(Error::lua)?;
      let prompt = self.lua.create_table().map_err(Error::lua)?;
      prompt.raw_set("section", section).map_err(Error::lua)?;
      prompt.raw_set("list", list).map_err(Error::lua)?;
      globals.raw_set("prompt", prompt).map_err(Error::lua)
  }
  ```
- File and public API changes:
  - Parser:
    - [crates/promptforge-internal/parser/src/prompt.rs](crates/promptforge-internal/parser/src/prompt.rs): the `markdown` field and accessor.
    - [crates/promptforge-internal/parser/src/build.rs](crates/promptforge-internal/parser/src/build.rs): `collect_headings` already slices each heading's content from the end of its heading line to the start of the next heading of any level. `build_sections` sets `markdown: trim_leading_blank_lines(&h.content).trim_end().to_owned()` in its `result.push(Section { ... })` literal, where `h`, the current `Heading`, is still in scope.
    - [crates/promptforge-internal/parser/src/fence.rs](crates/promptforge-internal/parser/src/fence.rs): `trim_leading_blank_lines` becomes `pub(crate)`. Plain `trim()` would strip the first content line's indentation.
    - [crates/promptforge-internal/parser/src/test_support.rs](crates/promptforge-internal/parser/src/test_support.rs): `synthetic_section` sets `markdown: String::new()`.
    - `Section` is not on the facade surface, so `crates/promptforge/public-api.txt` stays the same.
  - Engine:
    - [crates/promptforge-internal/engine/src/heading_address.rs](crates/promptforge-internal/engine/src/heading_address.rs): `find_visible` and the adapter as above. The adapter's callers (`resolve_chain_target` in `engine/src/execute/scheduler/walk.rs`, `end_live_h1_at_jump` in `engine/src/execute/scheduler/h1.rs`, and `resolve_jump_target` in `walk_target.rs`) don't change. Module docs name `prompt.section` and `prompt.list` in place of `list_from_section` and say `find_visible` is the one lookup.
    - [crates/promptforge-internal/engine/src/execute/walk_target.rs](crates/promptforge-internal/engine/src/execute/walk_target.rs): `read_visible` replaces `list_items_from_visible`; module docs. `home_without`'s doc comment keeps only its first paragraph. Its second describes a fanout arm whose slice lacks the caller, and no such slice exists any more: a spawned arm runs on the worker's own slice (`tasks.rs` hands `target_section.slice` to `start_chain`), and both callers of `visible_sections` pass the section's own slice. A doc-only change; the code stays as it is.
    - [crates/promptforge-internal/engine/src/execute/section_context-construct.rs](crates/promptforge-internal/engine/src/execute/section_context-construct.rs): in `new` and `new_live_h1`, `list_callback` becomes `read`, calling `read_visible`. The visible set still moves into one closure. Docs (five mentions).
    - [crates/promptforge-internal/engine/src/execute/section_vm.rs](crates/promptforge-internal/engine/src/execute/section_vm.rs): `setup_section_vm` takes `read`; docs (two mentions).
    - [crates/promptforge-internal/engine/src/lua.rs](crates/promptforge-internal/engine/src/lua.rs): re-export `SectionText`.
  - Lua crate:
    - [crates/promptforge-internal/lua/src/handles.rs](crates/promptforge-internal/lua/src/handles.rs): `SectionText`, beside `resolve_section_target`, and the module doc's first line, which today names only tool bindings, the tool set, and the output kind, names it too; [lib.rs](crates/promptforge-internal/lua/src/lib.rs) adds it to `pub use handles::{...}`.
    - [crates/promptforge-internal/lua/src/vm/install.rs](crates/promptforge-internal/lua/src/vm/install.rs): `install_prompt_table` replaces `install_list_global`; the two install functions take `read`; every doc mention of `list_from_section` names the `prompt` reads instead.
    - [crates/promptforge-internal/lua/src/globals.rs](crates/promptforge-internal/lua/src/globals.rs): `("list_from_section", ...)` becomes `("prompt", Reserved::EngineGlobal)` between `plugins` and `prose`; the array stays 61.
    - [crates/promptforge-internal/lua/src/collection.rs](crates/promptforge-internal/lua/src/collection.rs): the error becomes "fanout's second parameter is a collection; for a list section use prompt.list(heading), which returns nil when no visible section has that heading", plus its doc comment and test assertion. A message string only.
  - Test stubs, return type only (`Ok(None)`, or the existing `Err` reworded to "prompt reads are not needed here"), under `crates/promptforge-internal/`: `lua/src/tests.rs`, `lua/src/prelude-tests.rs`, `lua/src/globals-tests.rs`, `lua/src/tests/shared_replay.rs` (two sites), `engine/src/lua/tests.rs`, and `engine/src/lua/tests/globals.rs`. The two engine stubs bind a local named `list_callback`; it becomes `read`.
  - Docs, [../promptforge-design/design/architect-promptforge-prompting-language.md](../promptforge-design/design/architect-promptforge-prompting-language.md), in the separate `promptforge-design` repository that sits beside `promptforge` and is committed on its own:
    - line 48 (the L index line) drops `list_from_section(heading)`; line 52 (the P line) adds `prompt.list(heading)` and `prompt.section(heading)` after `pfanout` and before `promptforge: 0`, where they sort
    - line 92: the whole sentence is corrected, not just the read. `call`, `jump`, and `list_from_section` leave the list of forms it calls unavailable in H1, which keeps `call_async` and the deferred `execute` and `execute_async`, and the sentence says `call`, `jump`, and both `prompt` reads work in H1 over the whole top-level slice
    - line 125, the deferred `markdown` note, says heading-resolved reads live in `prompt` and `markdown` stays for parsing arbitrary text
    - lines 362 to 366: the `list_from_section(heading)` entry becomes entries for `prompt.section(heading)` and `prompt.list(heading)`, each opening with an `**Implementation status:**` line like its neighbors, then the same Effect, Return, Failure, Example format. The status lines describe behavior without naming the removed global, because the language is at `promptforge: 0`, where removed forms simply stop existing, and the exit search over the spec must find no `list_from_section`: `prompt.list` reads "Active: the list-section read, now `nil` on a miss", and `prompt.section` reads "Active: new in this plan"
- Data, persistence, failure, security, and privacy constraints:
  - Nothing is persisted, and no event or effect changes. A read is a pure function of the prompt and the caller's visible set, so replay is unaffected.
  - No scheduler, task runtime, Harness, or public API code changes.

</implementation-contract>
<verification-contract>

## Testing Plan

The test suite gets smaller. One parser test pins the span, one unit test pins the lookup's miss, and one engine file covers both reads end to end. That file has three tests over one shared document, replacing six, and three tests elsewhere that only repeat coverage are deleted. Suites that already use the list read keep running under the new name.

- Unit:
  - `crates/promptforge-internal/parser/src/tests/headings.rs`, one new test, `section_markdown_is_the_sections_own_content`. One prompt checks three values:
    - a section whose content has an indented first line, a Lua fence, a `---` break, and `{{ x }}` comes back exactly: heading line excluded, surrounding blank lines trimmed, stopping at its child heading. The `---` has a blank line above it: directly under a text line it is a setext heading underline (see `rule_ranges` in `fence.rs`), which would turn that line into a new section
    - the child's own Markdown
    - `""` for an empty section
  - `crates/promptforge-internal/engine/src/heading_address-tests.rs`, one new test, `find_visible_returns_none_for_an_address_not_in_the_set`: a missing name and a level mismatch are `Ok(None)`, and the exact address is `Some`. The seven existing `resolve_sibling` tests stay, and they now cover `find_visible`'s error paths through the adapter.
  - Reserved names need no new test: `crates/promptforge-internal/engine/src/lua/tests/globals.rs` already compares `RESERVED_NAMES` with a set-up VM's globals in both directions, so it pins `prompt` present and `list_from_section` absent.
- Integration and end-to-end:
  - Engine test paths in this list are relative to `crates/promptforge-internal/engine/src/execute/tests/`. That directory holds two different `exec_flow.rs` files: `exec_flow.rs` at its top, and `suite/exec_flow.rs`, which declares the `suite/exec_flow/` modules and defines the `flow_prompt!` macro (it prepends only frontmatter).
  - `suite/exec_flow/list_from_section.rs` becomes `suite/exec_flow/prompt_reads.rs`, with its module line and doc in `suite/exec_flow.rs`. One shared document, written once as a `const` and joined after each test's `## Main` block at runtime (the `[..].concat()` form `suite/exec_flow/fanout_arms.rs` uses, since `flow_prompt!` takes only a literal), holds:
    - `### Kid`, a child of `## Main`, with a grandchild `#### Grand`
    - a numbered `## Nums`
    - a raw `## Rules` whose body is its own `const RULES`, spliced into the document: a Lua fence, `{{ args }}`, and a `---` with a blank line above it. `RULES` starts on a non-blank line and ends without a trailing newline or space, because the read trims both ends and the test compares byte for byte; the document supplies the blank lines around it
    - an empty `## Empty`
    - `## Other`, with a niece `### Niece`
  - Every test's `## Main` ends with a scalar `return`, which ends the run, so the walk never reaches the shared sections and `## Rules`'s fence never runs. Each Lua `assert` includes a message naming its case, so a failure in the shared loop says which one.
  - The three tests:
    - `prompt_reads_resolve_over_the_visible_set`: numbered items with markers stripped, a direct child's items, a list section's raw bullets, `""` for the empty section, and `nil` from both reads for a missing heading, the niece, the grandchild, and the caller, checked in one Lua loop. `## Main` then returns `prompt.section('## Rules')`, and Rust compares the run's output to `RULES` with `assert_eq!`. The expected text is never written inside a Lua string: a line that is exactly the closing fence marker would end `## Main`'s own fence, because the fence scanner closes at the first such line.
    - `prompt_reads_raise_only_for_mistakes_in_the_call`: under `pcall`, a bare-name heading and a non-string argument raise from both reads, and `prompt.list('## Rules')` raises "has no pre-parsed items". A `call` to a missing heading still raises, and its listing (the text after "available sections:") names a sibling and a direct child but not the caller, the niece, or the grandchild. That assertion keeps all three of the old not-found-listing checks; no other engine test asserts what the listing leaves out.
    - `prompt_reads_work_in_h1_and_the_shared_library`: the H1 block reads a top-level section and list and gets `nil` for `### Kid`. It asserts and returns nothing, because a scalar return from H1 ends the run before `## Main` executes. A `lua shared` library reads `prompt.list('### Kid')` into a global at load time, so the replay resolves over each VM's own visible set: the H1 block sees that global as `nil`, and `## Main`, whose child `### Kid` is, sees its items (`suite/exec_flow/store_failures.rs` shows how a test writes the `lua shared` fence)
  - Kept under the new name, test function names included (`list_from_section_inside_a_fanout_arm_reads_items` becomes `prompt_list_inside_a_fanout_arm_reads_items`, and its doc comment, which describes the arm's visible set as "the set the worker was resolved from, minus the worker, plus its children", says instead that the arm resolves like any section: the worker's siblings minus the worker, plus its children): `suite/exec_flow/fanout_arms.rs` reading items inside an arm, `suite/exec_flow/fanout_collections.rs` feeding `fanout` from `prompt.list`, `live_infer.rs`, `model_and_reply-globals.rs`, and the fixtures `crates/promptforge-internal/engine/tests/prompts/execution/fanout-basic.md`, `fanout-epilog.md`, `fanout-store-writes.md`, and `fanout-arm-failure.md`.
  - Changed for the `nil` rule:
    - the list half of `suite/exec_flow/fanout_arms.rs`'s `call_and_list_inside_a_fanout_arm_reject_non_visible_sections` asserts `prompt.list('## Parent') == nil`; its doc comment and `expect` message stop saying both error not-found
    - `suite/exec_flow/fanout_collections.rs`'s two-string test becomes `fanout_string_second_parameter_errors_pointing_at_prompt_list` and asserts on `prompt.list`
  - Deleted as redundant:
    - `exec_flow.rs` `list_from_section_ambiguous_error_is_loud`: `heading_address-tests.rs` `resolve_sibling_rejects_more_than_one_match` pins the same guard
    - `suite/exec_flow/h1_control.rs` `list_from_section_from_h1_to_an_unknown_section_fails_the_run`: a miss no longer fails, and `prompt_reads_work_in_h1_and_the_shared_library` covers H1
    - `scheduler/live_h1/control_flow.rs` `list_from_section_works_on_the_h1`: covered by the same H1 test
  - Module docs that list `list_from_section` are updated in `exec_flow.rs`, `suite/exec_flow.rs`, `suite/exec_flow/h1_control.rs`, `suite/exec_flow/fanout_arms.rs`, and `scheduler/live_h1/control_flow.rs`, and the doc comment on the `suite/exec_flow/fanout_collections.rs` two-string test.
- Regression, security, and performance:
  - The `call` and `jump` suites pass unchanged, which pins that the adapter kept their behavior and messages.
  - A search over `crates/` and the language spec finds no `list_from_section` outside history files.
  - The docs-claims test passes.
- Exit criteria:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - both clippy runs with `CARGO_BUILD_WARNINGS=deny`: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo fmt --all --check`
  - the docs build (`cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`) and the facade docs build (`cargo doc -p promptforge --no-deps`), both with `RUSTDOCFLAGS="-D warnings"`
  - `cargo +nightly-2026-09-05 xtask api --check`; the nightly is the one pinned in `crates/build-xtask/src/api/toolchain.rs`, so use whatever that file names if it has moved
  - `cargo test -p build-xtask`
  - the docs-claims test: `node --test test/docs-claims.mjs`, run from `crates/workshop/ui` (`npm test` there runs it with the rest of the UI tests)
  - `rg -n list_from_section crates` and `rg -n list_from_section ../promptforge-design/design/architect-promptforge-prompting-language.md` both print nothing. This is the only gate that catches a leftover test name or doc comment.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - `prompt.section(heading)` returns a section's own raw Markdown: no heading line, no subsections, fences and `---` included, placeholders unrendered. Rationale: it is exactly what the section parses into blocks, and raw text is the only form in which Lua fences survive. User: "I want something like `section('### Instructions')` to return the section's markdown in its entirety including any fenced blocks or areas delineated with ---", then chose "Its own content only (no heading line, no subsections)".
  - Heading resolution follows `list_from_section`: siblings and direct children only, never grandchildren. Rationale: every heading surface agrees on what a section may name, and a miss can't expose deeper structure. User: "I want the scope of the passed section heading to follow the scoping rules of list_from_section. You can only name a sibling or a child" and "I agree. This is a good scoping rule".
  - The reads live in a `prompt` namespace, `prompt.section` and `prompt.list`, and `call`, `jump`, `prose`, and `sys` stay where they are. Rationale: one reserved name, the same namespace convention as `tools` and `models`, and a clear split from the deferred `markdown` namespace. A local named `prompt` shadows the namespace in its scope, an accepted tradeoff. User: "I like prompt.list and prompt.section".
  - Both reads return `nil` when the heading doesn't exist or names a section outside the caller's visible set. Rationale: a read is naturally optional, a heading used as an index needs a miss that isn't an error, and an empty visible section stays distinct as `""`. User: "prompt.section(x) should return nil if x doesn't exist or names an out of scope section", then "prompt.list should return nil. this lets you use section headings as indexes for example" with sections `### anthropic`, `### openai`, and `### gemini`, where `prompt.list("### grok")` gives `nil`.
  - `list_from_section` is removed outright, with no stub. User: "it should remove list_from_section".
  - One function, `find_visible`, answers every heading lookup, and two adapters decide what a miss means: `nil` for the reads, today's not-found error for `call` and `jump`. Rationale: every heading surface already searches only the visible set through `resolve_sibling`, so splitting its not-found branch out gives one lookup with no new rule. "Out of scope" and "doesn't exist" are one answer, because telling them apart would need the whole document and would let a miss reveal what the visible-set rule hides. User: "I want to make sure that there's only one function that calculates the scope, right? There should just be one utility function, given a section name, it tells you, you know, it gives you the section, or it tells you that it's out of scope, or it tells you that it, that it doesn't exist, maybe, something."
  - One `read` callback returning `SectionText` replaces the list callback, rather than a second callback beside it. Rationale: no signature gains a parameter, the Engine shares no visible set between closures, the seven test stubs change only their return type, and the "no pre-parsed items" check moves to the `prompt.list` surface where its error belongs. User: "find the minimum change. Can we reuse existing code? Can any existing code be removed?" and "make this a beautiful, crisp, concise, scoped change that reduces technical debt rather than increasing it".
  - The tests consolidate into one file of three tests over one shared document, and three tests that only repeat coverage are deleted. User: "How do we make the tests comprehensive and compact without bloating them, right? We want to avoid bloat."
  - The plan stays out of task execution. User: "we dont want to touch anything related to task execution". The only contact is text: `list_from_section` calls inside `fanout` test prompts and fixtures are renamed, one `fanout` test asserts `nil`, and the `fanout` collection error names `prompt.list`.
  - Defaults chosen in planning without discussion, open to override:
    - both reads raise for a malformed heading and a non-string argument, and return `nil` only for no match
    - `prompt.list` still raises for a visible section with no pre-parsed items, and the message names the heading the author wrote, trimmed as `resolve_sibling`'s messages trim it, rather than the bare section name
    - `resolve_sibling` keeps its name, since its other callers live in the scheduler
    - the new accessor and field are `markdown`, not `source`, because `LuaProgram::source` in the same crate means Lua code
    - `SectionText` derives no `Default`, since nothing constructs an empty one
- Rejected alternatives:
  - Two callbacks, one per read, sharing an `Arc` visible set. Reason: parallel plumbing through three layers and a new parameter at every call site. No revisit.
  - Telling "out of scope" apart from "doesn't exist". Reason: it needs the whole document, both read as `nil` anyway, and it would leak structure. No revisit.
  - Removing the lookup's ambiguity check. Reason: a parsed prompt can't reach it (the parser rejects duplicate sibling names, a section's children sit one level below its siblings, and H1's set is the top-level slice alone; fanout arms and `call` chains run on the target's own slice through `resolve_chain_target`, so they follow the same rule), but it also guards `call` and `jump`, which this plan leaves alone. Revisit in a cleanup of those paths.
  - Renaming `resolve_sibling`, which resolves children too. Reason: its callers are in the scheduler. Revisit with the H1 cleanup under Deferred.
  - Returning the heading line or the whole subtree from `prompt.section`. Reason: the user chose the section's own content. Revisit if authors need a subsection's text with its parent's.
  - Letting a section name its grandchildren. Reason: it breaks the one scoping rule. Revisit as a one-level-per-step path form if authors need it.
  - A bare `section` global, or a `doc` or `document` namespace. Reason: the user chose `prompt`. Revisit if shadowing by locals named `prompt` causes bugs.
  - Moving `sys.section_name` and `sys.section_count`, or `call`, `jump`, and `prose`, under `prompt`. Reason: `sys` is the sealed run-metadata table, `call` and `jump` move through the document rather than read it, and `prose` is the current block's text. No revisit.
  - Raising on a miss, or returning `nil` for a malformed heading too. Reason: the user chose `nil` for misses, and a malformed heading is a mistake in the call. Revisit if authors want a strict read; `assert(prompt.section(h), msg)` is the one-line form.
  - Keeping `list_from_section` as a deprecated alias. Reason: the user chose removal. No revisit.
- Assumptions, risks, and notes:
  - Assumption: no Plugin prelude in `crates/` defines a `prompt` or `section` global (searched at planning time), so the new reserved name collides with nothing.
  - Assumption: `list_from_section` already works from H1, though the language spec's line 92 says it doesn't; `crates/promptforge-internal/engine/src/execute/tests/scheduler/live_h1/control_flow.rs` pins it today. The same sentence also calls `call` and `jump` unavailable in H1, which is wrong too: `resolve_chain_target`'s H1 branch in `engine/src/execute/scheduler/walk.rs` and `end_live_h1_at_jump` in `engine/src/execute/scheduler/h1.rs` implement both. This plan rewrites that sentence anyway, so it corrects all three forms rather than leave a known-false claim in a sentence it edits.
  - Risk: because both reads return `nil` on a miss, a misspelled heading fails silently and surfaces later, for example as "attempt to concatenate a nil value" or as the `fanout` collection error. Authors who want a strict read write `assert(prompt.section(h), 'missing section: ' .. h)`.
  - Risk: authors who name a local `prompt` hide the namespace for the rest of that scope.
  - Note: the spec edit is committed in `promptforge-design` (`c:\Users\Vinnie\cursor\promptforge-design`) as its own commit, staging only the spec file and none of that repository's other untracked files. It goes with the step that removes `list_from_section`, which the spec describes, so no step's `promptforge` commit holds only a plan marker. Sub-agents leave the spec edit unstaged like any change; the session running the step commits it after the coding sub-agent returns and before that step's `promptforge` commit, and amends it after any later round that changes it. Nothing is pushed.
  - Note: `SectionText` copies both the Markdown and the items on every read, even though each read uses one. Section text is small, and one callback is simpler than two.
  - Note: repository paths are relative to the `promptforge` repository unless they start with `../`, and line numbers are from planning time, so search by name if they have moved. History files that mention `list_from_section` stay as they are: `vibe/` in `promptforge`, and `design-prev/`, `chats/`, and `research/` in `promptforge-design`, along with the prose-lint word list in `promptforge-design/saved-docs/cicerone/scripts/checks.py`.

### Deferred and Out of Scope

- Deferred: one helper for H1's visible set. The top-level slice is spelled out in three places under `crates/promptforge-internal/engine/src/execute/`: `section_context-construct.rs`, `scheduler/walk.rs`, and `scheduler/h1.rs`; folding them together touches the scheduler. Revisit in a change that already edits the `call` and `jump` paths.
- Deferred: grandchild addressing as a one-level-per-step path form. Revisit if authors need it.
- Deferred: a `markdown` namespace for parsing arbitrary text. Revisit when a prompt needs to parse text it didn't get from its own sections.
- Out of scope: task execution of any kind, including `tasks`, `fanout` behavior, subagents, the scheduler, and tool-call dispatch.
- Out of scope: the Harness, the package registry, and any Host-facing API.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (`crates/gateway/app` is the workspace's only default member, so plain `cargo build` builds just the gateway); the desktop app is an explicit `cargo build --locked -p workshop`. Build scripts bundle the UIs into `OUT_DIR`, so a fresh clone first runs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui`, as `.github/workflows/ci.yml` does.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-filter>`, dropping `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. A single JS test file runs with `node --test <file>` from its package directory.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features` for every crate except the three workshop crates above, which run without `--all-features`; CI adds separate runs for `workshop-workspace --all-features` and `workshop-server --features headless`. JS packages: `npm test --workspace <ui|look|platform>` from `crates/workshop`, or `npm test` from `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. The first run includes `build-xtask`, whose tests are the boundary and structural checks (`cargo xtask tidy` prints the same report). Also run `node --test crates/workshop/ui/test/docs-claims.mjs` (wording scan, no `npm ci` needed), `npm test --workspaces --if-present` in `crates/workshop`, and `npm test` in `crates/gateway/config-ui/ui`. Nextest config is `.config/nextest.toml`: 60 s slow timeout, terminated after 3 periods, and the STT crates run in a capped `heavy` group.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, both with `CARGO_BUILD_WARNINGS=deny` (in PowerShell, `$env:CARGO_BUILD_WARNINGS='deny'`). Also `cargo check -p gateway --no-default-features` for the headless build shape (the only allowed standalone `cargo check`), `npm run typecheck --workspaces --if-present` in `crates/workshop`, `npm run typecheck` in `crates/gateway/config-ui/ui`, and `cargo deny check` plus `cargo hakari verify` for dependency policy. The `.githooks/pre-push` hook runs the wording scan, the headless check, the first clippy run, and `cargo deny` when installed.
- Formatter check command: `cargo fmt --all --check` (stable toolchain per `rust-toolchain.toml`, `style_edition = "2024"` in `rustfmt.toml`; `.githooks/pre-commit` runs it).
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"`. Under the same flags CI also runs `cargo doc -p promptforge --no-deps` and `cargo doc -p harness --no-deps` with default features, and `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`. Facade surface: `cargo +nightly-2026-09-05 xtask api --check` against the committed `crates/promptforge/public-api.txt` (the nightly is pinned in `crates/build-xtask/src/api/toolchain.rs`; `--bless` rewrites the listing and refuses while violations remain), plus `cargo nextest run --locked -p build-xtask --run-ignored only` on that nightly for the rustdoc JSON fixtures.
- Test placement and naming conventions: Unit tests live in a sibling file wired with `#[cfg(test)] #[path = "<stem>-tests.rs"] mod tests;` (for example `crates/promptforge-internal/lua/src/handles.rs` beside `handles-tests.rs`), or in a `tests/` subdirectory of the module once it has three or more test files (for example `crates/promptforge-internal/engine/src/execute/tests/`). Integration tests compile as one binary per crate from `tests/it/main.rs` (most crates) or `tests/suite/main.rs` (`promptforge`, `harness`), with shared helpers in `support.rs` and one file per topic (`cancel.rs`, `replay.rs`, `vfs.rs`). Test function names are snake_case sentences describing the behavior (`a_run_can_move_between_threads_between_calls`). Plugin crates enable `promptforge-plugin`'s `test-support` feature from `[dev-dependencies]` only, which `build-xtask`'s `test_support_leak` guard enforces. Doctests are banned in every workspace crate (`build-xtask` `no_doctests`). `build-xtask` fixtures that need rustdoc JSON are `#[ignore]`d on stable. JS tests are `node:test` files in `test/**/*.mjs` (workshop `ui`, `look`, `platform`) and `src/**/*.test.mjs` (workshop `ui` and the gateway config UI).
- Directory map: `crates/` holds every crate. `promptforge` is the Engine facade (with `public-api.txt`), and the manifestless `promptforge-internal/` container holds the private Engine crates `types`, `vfs`, `parser`, `lua`, `model-client`, and `engine`. `promptforge-plugin` is the Plugin contract, implemented by `plugin-web`, `plugin-mcp`, and `plugin-user-input`. `harness` is the Harness facade, `harness-internal/runner` its private runner, and `harness-gateway-client` the gateway client. `gateway/` holds the gateway family (`app` binary, `config`, `config-ui`, `local`, `routing`, `protocol`, `cloud-providers`, `web-search`, `logging`, `progress`, and the `stt/` speech subsystem with `api`, `engine`, `backend-whisper`, `whisper-ffi`); `gateway-api-types` and `gateway-api-discovery` are its public pair. `workshop/` holds the Workshop Host: the `desktop` Tauri app, `server`, `server-api`, the tiered library crates, and the `ui`, `look`, `platform` npm workspaces. Also in `crates/`: `shared-error-source`, `shared-loopback`, and `shared-ui` (a TypeScript and CSS package, not a crate); `build-*` tooling (`build-xtask` for structural checks and `cargo xtask`, `build-workshop` behind `cargo workshop`, `build-ceiling` for the file-size limit, `build-ui`, `build-user-guide`, `build-llama-cuda`); and `workspace-hack` (cargo-hakari, configured in `.config/hakari.toml`). Top level: `guide/books/` (mdBook books `language`, `gateway`, `workshop`) with `guide/chrome/` and `guide/landing/` site assets, `prompts/` (sample prompts), `tools/` (Node scripts for gateway sidecar staging and live TTS, with tests), `.github/workflows/` (CI, release, nightly), `.githooks/`, `.cursor/rules/` (`workshop-architecture.mdc`, `workshop-spa.mdc`), `vibe/` (past plans, tracked), `local/` (gitignored machine-local config and fixtures), and `images/`.
- Component boundaries: `cargo test -p build-xtask` enforces these (`crates/build-xtask/src/product.rs` and `tidy.rs`). Engine crates (`promptforge*`) depend on no gateway, workshop, Harness, or Plugin crate, and their manifests may not name `tokio`, `tokio-util`, `async-trait`, or `reqwest` outside `[dev-dependencies]` (`engine_deps.rs`). Inside the Engine, `types` and `vfs` are leaves (`vfs` has no dependencies at all), `model-client` depends on `types`, `lua` on `types`, `vfs`, and `model-client`, `parser` on `lua` and `types`, `engine` on all five, and the `promptforge` facade on all six. `promptforge-plugin` depends on `types` and `vfs`. Plugin crates depend only on `promptforge-plugin`, `shared-*`, `workspace-hack`, and outside libraries. Harness crates name only `promptforge`, `promptforge-plugin`, and `workspace-hack` outside their family, except that `harness-gateway-client` may name `plugin-web` to implement its `SearchProvider`. Gateway crates depend on no Engine, Harness, or workshop crate. Workshop crates reach the gateway only through `gateway-api-types` and `gateway-api-discovery`, and the Engine and Harness only through `promptforge`, `promptforge-plugin`, `harness`, and `harness-gateway-client`. Containers are private: only `promptforge` and `promptforge-plugin` may enter `promptforge-internal/`, only `harness` may enter `harness-internal/`, and only `gateway-stt` is public within `gateway/stt/`. Workshop tiers run vocabulary (`protocol`, `registry`, `support`), then services (`gateway`, `menu`, `status`), then features (`agents`, `run-log`, `user-state`, `workspace`), then `workshop-server`, each depending only downward. The `workshop` desktop app depends only on `workshop-server-api`, which depends only on `workshop-server`. `shared-*` crates depend on no product crate.
- Conventions summary: The four defined terms in `AGENTS.md` (Engine, Harness, Host, Plugin) are capitalized with one meaning each, and Engine crates say "the caller" and never mention the Host (`crates/workshop/ui/test/docs-claims.mjs` scans for this). All 52 crate manifests set `publish = false` and `[lints] workspace = true`, inherit `version`, `edition`, `license`, and `repository`, and take dependencies as `<dep>.workspace = true`; every crate except `promptforge-vfs` and `build-ceiling` depends on `workspace-hack`. Every crate's build script runs `build_ceiling::check()`, which fails any file over 500 lines. Manifests comment why each dependency, pin, or feature exists. Lints are strict: clippy `all` and `pedantic` deny, `unwrap_used` and `expect_used` deny outside tests, bare `#[allow]` is denied so suppressions use `#[expect(..., reason = "...")]`, `unsafe_code` is denied outside the allowlisted boundaries, `missing_docs` and `unreachable_pub` warn (fatal under the gate), and `clippy.toml` bans process-global installers outside binary entry points. Workshop, Harness, and Plugin crates must carry a `## Invariants` section in their crate docs. Source directories stay flat until a group reaches three files, using kebab siblings with `#[path]` below that. Comments explain only non-obvious constraints and cite upstream issue URLs for workarounds. Error messages are written for model consumption, naming required versus actual. JSON that reaches a recorder round-trips exactly, with sorted keys and `float_roundtrip`. Behavior changes ship with tests in the same change. Cargo features gate only real build constraints. Changes to the facade surface update `public-api.txt`. Commits are one-line imperative summaries (`Resolve model tool names by wire name only`), and a finished plan closes with `Close plan: <slug>`.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Keep each section's raw Markdown in the parser [completed]

- Component: section-markdown
- Component placement: first. It depends on nothing, and Step 3's `prompt.section` reads `Section::markdown()`. The parser sits below the Engine in the crate graph, so it lands before the Engine-side lookup.
- Pieces: one, the `markdown` field with its accessor and test. Construction: sequential, since there is only one piece.
- Work item: `parser-markdown`
- Changes, under `crates/promptforge-internal/parser/src/`:
  - `prompt.rs`: `pub(crate) markdown: String` on `Section` and `pub fn markdown(&self) -> &str`, with the doc comment from the Technical Design
  - `fence.rs`: `trim_leading_blank_lines` becomes `pub(crate)`; plain `trim()` would strip the first content line's indentation
  - `build.rs`: `build_sections` sets `markdown` in its `Section { ... }` literal from the current `Heading`'s `content`, through `trim_leading_blank_lines` and then `trim_end`
  - `test_support.rs`: `synthetic_section` sets `markdown` to `String::new()`
  - `crates/promptforge/public-api.txt` stays the same, since `Section` is not on the facade surface
- Tests: `crates/promptforge-internal/parser/src/tests/headings.rs` gains `section_markdown_is_the_sections_own_content`, one prompt checking three values: a section with an indented first line, a Lua fence, a `---` with a blank line above it (directly under text it would be a setext underline), and `{{ x }}`, returned byte for byte without its heading line and stopping at its child heading; the child's own Markdown; and `""` for an empty section
- Gate: `cargo nextest run --locked -p promptforge-parser --all-features`, the first clippy run with `CARGO_BUILD_WARNINGS=deny`, and `cargo fmt --all --check`
- Commit: `Keep each section's raw Markdown in the parser`

</step-1>

<step-2>

### Step 2: Split find_visible out of resolve_sibling

- Component: heading-lookup
- Component placement: second. It is logically independent of Step 1, but each gate builds the whole workspace, so in one checkout they run in sequence; running it beside Step 1 needs a separate worktree. Step 3's `read_visible` sits on `find_visible`.
- Pieces: one, the lookup and its adapter. Construction: sequential, since there is only one piece.
- Work item: `scope-lookup`
- Changes, under `crates/promptforge-internal/engine/src/`:
  - `heading_address.rs`: `pub(crate) fn find_visible` is today's `resolve_sibling` body without its not-found branch, returning `Ok(None)` when no visible section has the heading's exact level and name and keeping the malformed-heading and ambiguity errors; `resolve_sibling` becomes a thin adapter that turns `None` into today's "not found; available sections" error with unchanged text
  - `heading_address.rs` module docs say `find_visible` is the one heading lookup; their `list_from_section` mention changes in Step 3
  - the adapter's callers (`resolve_chain_target` in `execute/scheduler/walk.rs`, `end_live_h1_at_jump` in `execute/scheduler/h1.rs`, and `resolve_jump_target` in `execute/walk_target.rs`) don't change, and no source under `execute/scheduler/` is edited
- Tests: `heading_address-tests.rs` gains `find_visible_returns_none_for_an_address_not_in_the_set`: a missing name and a level mismatch are `Ok(None)`, and the exact address is `Some`. The seven existing `resolve_sibling` tests stay as they are and pin the adapter's messages.
- Gate: `cargo nextest run --locked -p promptforge-engine --all-features`, the first clippy run with `CARGO_BUILD_WARNINGS=deny`, and `cargo fmt --all --check`
- Commit: `Split find_visible out of resolve_sibling`

</step-2>

<step-3>

### Step 3: Replace list_from_section with prompt.section and prompt.list

- Component: prompt-namespace
- Component placement: last. It needs `Section::markdown()` from Step 1 and `find_visible` from Step 2, and it is the only component prompt authors see.
- Pieces: the table, the removal, the tests, and the language spec. Construction: joint. Removing `list_items_from_visible` and the `list_from_section` global, and moving every user of both (including the `exec_flow.rs` test that calls `list_items_from_visible` directly), must land together to keep the suite green, and the spec describes the removed global, so it lands with the removal.
- Work items: `prompt-change` and `docs`, closing with the full `verify` gate
- The table, under `crates/promptforge-internal/`:
  - `lua/src/handles.rs`: `SectionText { markdown, items }` deriving `Debug, Clone, PartialEq, Eq` (no `Default`), beside `resolve_section_target`, and the module doc's first line names it; `lua/src/lib.rs` adds it to `pub use handles::{...}`, and `engine/src/lua.rs` re-exports it
  - `lua/src/vm/install.rs`: `install_prompt_table` replaces `install_list_global` and builds `prompt.section` and `prompt.list` over one `Arc`-shared `read` callback; `prompt.list` raises "section `<heading>` has no pre-parsed items" with the trimmed heading; `install_scheduler_control_globals` and the test-only `install_control_globals` take `read` with `Send + Sync + 'static` bounds; every doc mention of `list_from_section` names the `prompt` reads
  - `lua/src/globals.rs`: `("list_from_section", ...)` becomes `("prompt", Reserved::EngineGlobal)` between `plugins` and `prose`, and the array stays 61
  - `engine/src/execute/walk_target.rs`: `read_visible` over `heading_address::find_visible` replaces `list_items_from_visible`; `home_without`'s doc comment keeps only its first paragraph (doc-only); module docs
  - `engine/src/execute/section_vm.rs`: `setup_section_vm` takes `read`; docs (two mentions)
  - `engine/src/execute/section_context-construct.rs`: `new` and `new_live_h1` pass a `read` closure calling `read_visible`, with the visible set still moving into one closure; docs (five mentions)
  - the seven test stubs change only their return type (`Ok(None)`, or the existing `Err` reworded to "prompt reads are not needed here"): `lua/src/tests.rs`, `lua/src/prelude-tests.rs`, `lua/src/globals-tests.rs`, `lua/src/tests/shared_replay.rs` (two sites), `engine/src/lua/tests.rs`, and `engine/src/lua/tests/globals.rs`; the two engine stubs rename their local `list_callback` to `read`
- The removal, under `crates/promptforge-internal/`:
  - `lua/src/collection.rs`: the error becomes "fanout's second parameter is a collection; for a list section use prompt.list(heading), which returns nil when no visible section has that heading", with its doc comment and test assertion
  - `engine/src/heading_address.rs`: module docs name `prompt.section` and `prompt.list`
  - every remaining `list_from_section` call, test function name, and doc comment under `engine/src/execute/tests/`: `live_infer.rs`, `model_and_reply-globals.rs`, `suite/exec_flow/fanout_collections.rs`, and `suite/exec_flow/fanout_arms.rs`, where `list_from_section_inside_a_fanout_arm_reads_items` becomes `prompt_list_inside_a_fanout_arm_reads_items` and its doc comment says the arm resolves like any section (the worker's siblings minus the worker, plus its children); module docs in `exec_flow.rs`, `suite/exec_flow.rs`, `suite/exec_flow/h1_control.rs`, `suite/exec_flow/fanout_arms.rs`, and `scheduler/live_h1/control_flow.rs`
  - the four fixtures under `engine/tests/prompts/execution/`: `fanout-basic.md`, `fanout-epilog.md`, `fanout-store-writes.md`, and `fanout-arm-failure.md`
- The tests, under `crates/promptforge-internal/engine/src/execute/tests/`:
  - `suite/exec_flow/prompt_reads.rs` replaces `suite/exec_flow/list_from_section.rs`, with its module line and doc in `suite/exec_flow.rs`. One shared `const` document (`### Kid` with `#### Grand`, a numbered `## Nums`, a raw `## Rules` spliced from its own `const RULES`, an empty `## Empty`, and `## Other` with `### Niece`) is joined after each test's `## Main` with the `[..].concat()` form from `suite/exec_flow/fanout_arms.rs`. Three tests, built as the Testing Plan describes: `prompt_reads_resolve_over_the_visible_set`, `prompt_reads_raise_only_for_mistakes_in_the_call`, and `prompt_reads_work_in_h1_and_the_shared_library`
  - the `nil`-rule edits: `call_and_list_inside_a_fanout_arm_reject_non_visible_sections` in `suite/exec_flow/fanout_arms.rs` asserts `prompt.list('## Parent') == nil`, with its doc comment and `expect` message no longer saying both error not-found; the two-string test in `suite/exec_flow/fanout_collections.rs` becomes `fanout_string_second_parameter_errors_pointing_at_prompt_list`, asserting on `prompt.list`, with its doc comment updated
  - the deletions: `list_from_section_ambiguous_error_is_loud` in `exec_flow.rs`, `list_from_section_from_h1_to_an_unknown_section_fails_the_run` in `suite/exec_flow/h1_control.rs`, and `list_from_section_works_on_the_h1` in `scheduler/live_h1/control_flow.rs`
- The language spec, `../promptforge-design/design/architect-promptforge-prompting-language.md`:
  - the L index line (line 48) drops `list_from_section(heading)`, and the P line (line 52) adds `prompt.list(heading)` and `prompt.section(heading)` after `pfanout` and before `promptforge: 0`
  - the H1 availability sentence (line 92) is rewritten: the unavailable list keeps `call_async`, deferred `execute`, and deferred `execute_async`, and the sentence says `call`, `jump`, and both `prompt` reads work in H1 over the whole top-level slice
  - the deferred `markdown` note (line 125) says heading-resolved reads live in `prompt` and `markdown` stays for parsing arbitrary text
  - the `list_from_section(heading)` entry (lines 362 to 366) becomes `prompt.section(heading)` and `prompt.list(heading)` entries, each opening with an `**Implementation status:**` line ("Active: new in this plan" and "Active: the list-section read, now `nil` on a miss"), then Effect, Return, Failure, and Example, without naming the removed global
- Gate, the full exit criteria from the Testing Plan: both nextest runs; both clippy runs with `CARGO_BUILD_WARNINGS=deny`; `cargo fmt --all --check`; the docs build and the facade docs build with `RUSTDOCFLAGS="-D warnings"`; `cargo +nightly-2026-09-05 xtask api --check` (or whichever nightly `crates/build-xtask/src/api/toolchain.rs` names); `cargo test -p build-xtask`; `node --test test/docs-claims.mjs` from `crates/workshop/ui`; and `rg -n list_from_section crates` plus `rg -n list_from_section ../promptforge-design/design/architect-promptforge-prompting-language.md`, both printing nothing
- Commits, two, spec first, per the Decision Record note: in `promptforge-design`, staging only the spec file, `Specify prompt.section and prompt.list in place of list_from_section`, committed by the session after the coding sub-agent returns and amended after any later round that changes it; then in `promptforge`, `Replace list_from_section with prompt.section and prompt.list`. Nothing is pushed.

</step-3>

</execution-plan>
