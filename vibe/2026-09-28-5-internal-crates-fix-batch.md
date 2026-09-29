---
name: Internal crates fix batch
overview: Fix the cancellation, host-backend, claims-ledger, and validation defects left in the six promptforge-internal crates with the smallest change that satisfies each finding, verified against HEAD a05d5cbd; record findings that turned out to be intended behavior as non-changes; then split every Rust file in the six crates under the 500-line ceiling and turn on its enforcement.
todos:
  - id: types
    content: "Types: Cancelled deregisters on drop; fold ModelCatalog::from_validated; event test gaps"
    status: pending
  - id: model-client
    content: "Model client: validating from_result/from_parts with their engine test and facade doctest callers; linear SSE scan; null error field; #[non_exhaustive] ClientError with wildcard arms; re-bless listing"
    status: pending
  - id: lua-cancel
    content: "Lua cancellation: skip the author xpcall handler under cancel; refuse __gc metatables and __mode on _G; tests"
    status: pending
  - id: lua-cleanup
    content: "Lua cleanup: pin var.k = nil; delete the unreachable Rust placeholder behind models.infer (models.infer itself stays); correct the run_store_op doc; crate docs"
    status: pending
  - id: vfs-host
    content: "VFS host: refuse dangling symlinks in contain; README sentence; tests"
    status: pending
  - id: vfs-claims
    content: "VFS claims: close the three verified gaps (nested subtrees, subtree vs parent listing, created ancestors vs listings and globs); both-order tests"
    status: pending
  - id: dead-api
    content: Remove grep (VFS crate, facade re-exports, vfs.md, tests) and Environment::max_depth (engine, lib.md); re-bless listing
    status: pending
  - id: parser-frontmatter
    content: "Parser frontmatter: duplicate-capability refusal; optional-capability slot refusal; guide text; regenerate the combined guide; tests"
    status: pending
  - id: parser-structure
    content: "Parser structure: span doc correction; column-0 frontmatter closer; container headings; structural error locations; tests"
    status: pending
  - id: engine-surface
    content: "Engine surface: narrow the unused public surface with the bench and doctest paths; bench-only test-support docs; Requirements::notice and prelude-collision tests"
    status: pending
  - id: engine-scheduler
    content: "Engine scheduler: entered placeholder; drive.rs panic; shared Chain constructor; rename fanout.rs to heading_address.rs; tests"
    status: pending
  - id: housekeeping
    content: "Housekeeping: fold build-xtask product-* and api/listing-*; vfs manifest test catches the missing dependency table forms; audit-tag and history comment sweep"
    status: pending
  - id: split-small
    content: "Split to the ceiling and add the marker: promptforge-types, promptforge-model-client, promptforge-parser"
    status: pending
  - id: split-lua-tests
    content: "Split promptforge-lua src/tests.rs to the ceiling"
    status: pending
  - id: split-lua-rest
    content: "Split the rest of promptforge-lua to the ceiling and add the marker"
    status: pending
  - id: split-engine-source
    content: "Split the promptforge-engine source files and forward-tests.rs to the ceiling"
    status: pending
  - id: split-engine-exec-flow
    content: "Split promptforge-engine suite/exec_flow.rs to the ceiling"
    status: pending
  - id: split-engine-scheduler-tests
    content: "Split the promptforge-engine scheduler/ test files to the ceiling"
    status: pending
  - id: split-engine-other-tests
    content: "Split the remaining promptforge-engine test files to the ceiling and add the marker"
    status: pending
  - id: split-vfs-handle
    content: "Split promptforge-vfs src/handle.rs to the ceiling"
    status: pending
  - id: split-vfs-rest
    content: "Split the rest of promptforge-vfs to the ceiling, add the marker, and pass the full exit gates"
    status: pending
isProject: false
---

# Internal crates fix batch

<product-contract>

## Product Requirements

The six crates in `crates/promptforge-internal/` have places where a cancelled run can keep running, where a host backend can create a file outside its root, and where the VFS claims ledger misses real races, plus a set of smaller defects. This plan fixes each with the smallest change that satisfies it, and every fact it relies on was checked against commit `a05d5cbd` on `master`. The complete list of what to change is under Technical Design; nothing outside this document is needed to carry it out. Candidate changes that turned out to be documented behavior are recorded as non-changes, and changes that would have committed to design choices nobody made were put to the user and settled. The plan then splits every Rust file in the six crates under the 500-line ceiling and turns the ceiling check on for them. All paths are relative to the root of the promptforge repository, and line numbers are at `a05d5cbd`.

- Problem and users:
  - Host operators, cancellation. An author `xpcall` whose message handler loops keeps a cancelled run alive.
    - The engine runs Lua 5.5.0, from the `lua-src` crate at version 550.1.1 (`Cargo.lock`, lines 3710 to 3712), selected by the `lua55` and `vendored` mlua features in the root `Cargo.toml`. The C files cited below are in that crate's `lua-5.5.0/` directory, inside the Cargo registry (for example `~/.cargo/registry/src/<index>/lua-src-550.1.1/lua-5.5.0/`), not in this repository.
    - Lua 5.5.0 runs the current message handler with hooks disabled when the error was raised from inside a hook (`ldo.c`, lines 457 to 463; `ldebug.c`, lines 840 to 847). The cancellation error is raised from the instruction hook (`crates/promptforge-internal/lua/src/hardening.rs`).
    - The shim's `protected_xcall` (`crates/promptforge-internal/lua/src/__impl_coro.lua`, lines 92 to 99) calls the author handler, and only afterwards does `xpcall_outcome` (lines 87 to 90) re-raise under cancellation.
    - A table finalizer (`__gc`) that loops has the same effect, because Lua runs finalizers with hooks disabled (`lgc.c` lines 977 to 986).
    - `__close` handlers are not affected: pcall recovery restores hooks before closing (`ldo.c` lines 810 to 811 and 1085 to 1092).
  - Hosts that mount `HostBackend`. `append` through a dangling symlink creates a file outside the root.
    - `contain` (`crates/promptforge-internal/vfs/src/host.rs`, lines 130 to 159) walks up with `Path::exists()`, which follows links. A dangling link therefore looks absent and is re-appended lexically.
    - `append` (lines 535 to 551) then opens it with `create(true)`. `write` is safe because `atomic_write` (lines 191 to 226) renames a temp file over the link.
    - No production host mounts `HostBackend`: the harness launches with `VfsRef::default()`, a memory store at `/` (`crates/harness-internal/sessions/src/runtime.rs`, lines 49 to 59). The backend is public on the facade.
  - Engine contributors, claims. Three claims-ledger gaps in `crates/promptforge-internal/vfs/src/handle.rs` let a real cross-task race pass undetected. No claims test covers any of them.
  - Host implementers, completions. `Completion::from_result` and `ToolCall::from_parts` (`crates/promptforge-internal/model-client/src/client/wire-canned.rs`, lines 16 to 47) are infallible constructors on the facade. They skip every check the live normalizer applies (`normalize.rs`, lines 125 to 177 and 187 to 268).
  - Contributors, file size. None of the six crates carries `//! ## Invariants`, so `cargo test -p build-xtask` never checks them against the 500-line ceiling (`crates/build-xtask/src/tidy.rs`: `INVARIANT_MARKER` at line 56, `file_ceiling_violations` at lines 177 to 198). 35 Rust files are over. Together they hold 35,092 lines, 17,592 of them beyond the ceiling, counted as `text.lines().count()`, the check's own count.
  - Smaller defects: listed under Technical Design.
- Goals:
  - No author Lua outlives cancellation through an `xpcall` message handler or a table finalizer.
  - `HostBackend` content operations refuse a dangling symlink, so none creates a file outside the root.
  - Each of the three claims gaps conflicts in both operation orders.
  - A frontmatter tool slot that names an optional capability is refused at parse time.
  - A capability declared more than once under frontmatter `capabilities:` is refused at parse time.
  - Host-built completions and tool calls are validated.
  - Two dead public facade items leave: grep, and `Environment::max_depth`.
  - `ClientError` becomes `#[non_exhaustive]`.
  - The smaller fixes listed under Technical Design land.
  - Every Rust file in `promptforge-engine`, `promptforge-lua`, `promptforge-parser`, `promptforge-vfs`, `promptforge-model-client`, and `promptforge-types` is at most 500 lines, and each crate carries the marker, so the ceiling stays enforced.
- Non-goals:
  - Changing documented behavior. Guarded `var` and frozen `argv` look empty to `pairs` and `#` by design (`guide/src/language/05-lua-environment.md`, lines 434 to 443; `guide/src/language/06-arguments.md`, lines 420 to 462).
  - Remapping parser spans to source offsets. No consumer outside the parser's own tests reads `ParseError::span()`, so only its documentation changes.
  - SSE CR-only line endings and multi-line `data:` joining. No test fixture uses either (`crates/promptforge-internal/model-client/src/client/stream-tests.rs`, lines 29 to 37; `read-tests.rs`, lines 54 to 61), and no backend is known to send them.
  - Merging the Lua store closures with `run_store_op`. They differ in behavior (see Technical Design), so only the doc that says otherwise changes.
  - The facade crate's marker, and applying the ceiling to Markdown and Lua shim files, which the check does not count.
  - Everything under Deferred and Out of Scope.
- Success criteria:
  - A looping `xpcall` handler around a looping body ends as `RunResult::Cancelled` once the cancel flag is set, and `setmetatable` refuses any metatable carrying `__gc`, so no author finalizer can run.
  - `append` through a dangling symlink inside a mounted host root is refused, and no file appears at the link's target.
  - Each claims gap has a test with two unordered tasks, in both orders, that ends in a claims conflict.
  - A prompt that binds a tool of an optional capability fails to parse with the message in Functional Specification.
  - A prompt that lists one capability twice under `capabilities:` fails to parse with the message in Functional Specification.
  - `from_result` and `from_parts` refuse the inputs listed in Functional Specification.
  - All six crates' `src/lib.rs` contain `//! ## Invariants`, and `cargo test -p build-xtask` passes.
  - Each crate's test count after its splits is no lower than before them, and the API listing diff contains only the changes File and public API changes names.
- Constraints:
  - Each fix is the smallest change that satisfies its finding. Any change beyond that, including a new public item, a changed contract, or a platform-specific rule, needs the user's decision first. User's words: "make sure we are not building out too much for example for the frontmatter compliance, without direct input from me. I dont want a decision like \"require the capability\" to unexpectedly turn into writing a lot of code that assumes a lot of things."
  - The engine stays sans-IO. Every existing behavior test keeps passing except the ones this plan changes on purpose.
  - Error messages state required versus actual, per `AGENTS.md`.
  - Splits follow the flat-directory rule in `AGENTS.md`. Moved items take the narrowest visibility that compiles (`pub(super)` or `pub(crate)`). No public path changes.
  - The facade firewall, the `#[doc(hidden)]` ban, and the API listing gate stay in force.
- Open questions: None

## Functional Specification

Authors see cancellation hold in two more constructs and two new refusals from `setmetatable`. A frontmatter tool slot that names an optional capability no longer parses. Hosts that mount `HostBackend` see dangling links refused, and host implementers see validating constructors. Contributors see the ceiling check fail on oversized files. Everything else is internal.

- Actors and workflows:
  - Prompt author:
    - `xpcall(f, handler)` still calls `handler` for ordinary failures. After cancellation, `handler` is not called and the run ends as interrupted.
    - `setmetatable(t, mt)` is refused when `mt` has a raw `__gc` field, for any table `t`. `setmetatable(_G, mt)` is also refused when `mt` has a raw `__mode` field.
    - A `tools:` slot whose tool id's capability is declared `optional: true` under `capabilities:` fails parsing. The capability is `ToolId::capability()`, the id's first two segments. Declaring an optional capability without a tool slot is unchanged. Calling its tools by full id through `tools.call` when the capability is present is unchanged too (`crates/promptforge-internal/engine/src/execute/scheduler/tool_call.rs`).
    - A `capabilities:` list that names the same capability id more than once fails parsing. This holds whatever the `optional` flags are, and whichever entry form each uses.
  - Host implementer: `Completion::from_result` and `ToolCall::from_parts` return a `Result`.
  - Host that mounts `HostBackend`: a content operation whose path passes through a dangling symlink is refused.
  - Contributor: `cargo test -p build-xtask` fails when a Rust file in one of the six crates exceeds 500 lines, with the check's existing message, `<path> has <n> lines, over the 500-line ceiling` (`crates/build-xtask/src/tidy.rs`, lines 190 to 193).
- Inputs and outputs:
  - Messages:
    - Optional-capability slot: `invalid frontmatter: tool alias '<alias>' names <tool id>, whose capability <capability> is declared optional; a tool slot requires its capability`.
    - Duplicate capability: `invalid frontmatter: capability <capability> is declared more than once under capabilities`.
    - `__gc`: `setmetatable: finalizers (__gc) are not available in the sandbox`.
    - `__mode` on `_G`: `setmetatable: weak tables (__mode) are not available for _G`.
    - Dangling link: `VfsError::PermissionDenied` with reason `<path> passes through a dangling symbolic link`.
  - API listing:
    - The signatures of `Completion::from_result` and `ToolCall::from_parts` change.
    - `ClientError` becomes `#[non_exhaustive]`.
    - `GrepQuery`, `GrepResults`, `GrepMatch`, `Access::grep`, `VfsAccess::grep`, `Op::Grep`, and `Environment::max_depth` are removed.
- States and validation:
  - Cancellation: in `protected_xcall`, the wrapper around a function handler checks `cancel_requested()` first. When it is set, the wrapper returns the failure unchanged without calling the author handler, and `xpcall_outcome` re-raises it.
    - A non-function handler already goes to the raw `xpcall`, which refuses it with its own argument error before any author code runs.
    - `run_local_tool` (`__impl_coro.lua`, lines 140 to 151) and `compact` (lines 221 to 231) call author functions through `raw_pcall`. That installs no message handler, and both already re-raise under cancellation.
    - The block guard's `raw_xpcall` (lines 343 to 355) uses the host's own `guard_handler`, not author code.
  - `__gc` is checked with `rawget(mt, "__gc") ~= nil` at `setmetatable` time. This matches Lua's rule that a table is marked for finalization only if its metatable has `__gc` when it is set.
  - Dangling link: while `contain` walks up, an entry for which `symlink_metadata` succeeds but `exists()` is false is a dangling link, and the operation is refused.
  - Claims: each new conflict uses the same happens-before comparison the neighboring checks apply to a recorded claim, so ordered accesses from the same task or across a join never conflict.
  - `ToolCall::from_parts` refuses a blank id, a blank name, and arguments that are not a JSON object. `Completion::from_result` refuses a tool-call result that is empty or has duplicate call ids. A text result is not validated, because the live path's empty-reply handling is not part of this constructor.
- Errors and recovery:
  - Every cancelled construct above ends the run as `RunResult::Cancelled`.
  - A read through a dangling symlink, which returns `VfsError::NotFound` today, returns `VfsError::PermissionDenied`.
  - A write to a dangling symlink, which today replaces the link with a regular file through `atomic_write`, is refused.
  - The validating constructors return the same `ClientError` variant the live normalizer raises for the same rule.
- Security and privacy behavior:
  - No new host-visible capability is added.
  - Cancellation still cannot interrupt a single long-running C library call, such as a pathological Lua string pattern match; see Deferred and Out of Scope.
- Acceptance criteria:
  - Every Success criterion holds, and every Testing Plan item passes.

</product-contract>
<implementation-contract>

## Technical Design

Each fix stays inside the crate that owns the behavior, and no crate or dependency is added. The entries below give, for each change, the current code at `a05d5cbd` and the minimal edit, so the implementer starts from verified locations. After the fixes, oversized files are split along their existing seams with no behavior change, and each crate gains the marker that puts it under the existing ceiling check.

- Architecture:
  - No module or crate boundary changes, apart from the file splits.
  - The facade changes only where File and public API changes says.
- Modules and interfaces:
  - `promptforge-lua` (`crates/promptforge-internal/lua/src/`):
    - `__impl_coro.lua`, `protected_xcall` (lines 92 to 99): the function-handler wrapper becomes "if `cancel_requested()` then return the failure, else return `handler(normalize_failure(failure))`". `cancel_requested` is already a chunk argument (lines 25 to 27). No other shim path needs a change; see Functional Specification.
    - `__impl_globals.lua`, `replace_metatable` (lines 90 to 108):
      - For any target, when `metatable` is a table with `rawget(metatable, "__gc") ~= nil`, raise the `__gc` message at level 2, like the function's other errors.
      - For target `_G`, also refuse `rawget(metatable, "__mode") ~= nil`.
      - `forward` (lines 72 to 80) stops copying `__mode` onto the guard.
      - `replace_metatable` is the only `setmetatable` authors can reach (`globals.rs`, lines 85 to 87). The chunk captures `rawget` before the sandbox strips it (line 16).
    - The unreachable Rust implementation behind `models.infer`. `models.infer` itself stays unchanged for authors. The function they call is the coroutine shim's `infer` (`__impl_coro.lua`, lines 101 to 117), which yields `{ op = "infer" }` to the scheduler. The shim assigns it over the Rust placeholder (lines 393 to 394) in `install_coro_shims`, which `setup_section_vm` runs before any author chunk or the shared library (`crates/promptforge-internal/engine/src/execute/section_vm.rs`, lines 139 to 163). The live H1 pass takes the same shim `infer`. The Rust placeholder only forwards to a `ModelsInferHook` that nothing ever sets, so if it were ever reached it would fail with "models.infer is not available outside section execution". Delete:
      - the Rust placeholder closure and `call_models_infer_hook` in `install_models` (`models.rs`, lines 57 to 72 and 351 to 354);
      - `ModelsInferHook` (`models-userdata.rs`, lines 15 to 23);
      - `clear_infer_hook` and its teardown call (`vm.rs`, lines 569 to 575 and 1052 to 1075);
      - `ModelsInferHook` in the re-export lines of `models.rs` (line 26) and `lib.rs` (line 57) and the `vm.rs` import list (line 16); `LuaModelHandle` stays in both re-exports.

      Nothing in production sets the hook, and the coroutine shim overwrites `models.infer` before any author chunk runs (`__impl_coro.lua`, lines 393 to 394). Update `the_models_namespace_has_no_bind` (`models-tests.rs`, lines 70 to 84), which asserts that `models.infer` is a function after `install_models`. Remove the stale "infer hook" comments in `crates/promptforge-internal/engine/src/execute/tests/suite/exec_flow.rs`.
    - Store doc: `host.rs` (lines 641 to 643) says `run_store_op` is the single implementation, which is not true. The doc is corrected; no code changes. The direct closures (lines 470 to 678):
      - run only during shared-library load, before `route_store_to_shims`;
      - share the VFS operation bodies with `run_store_op`;
      - additionally report lifecycle events, record store conflicts, and wrap `Error::store`.
    - `var.k = nil`: the `sys.rs` `__newindex` converts the value to JSON and back, then `raw_set`s it, so Lua nil round-trips to nil and removes the key. A test pins this. The code changes only if the test fails.
    - Crate docs in `lib.rs` mention capability preludes, `input`, and `tasks`.
  - `promptforge-vfs` (`crates/promptforge-internal/vfs/src/`):
    - `host.rs`, `contain` (lines 130 to 159): inside the walk-up loop, before stepping to the parent, refuse when `ancestor.symlink_metadata().is_ok()` and `!ancestor.exists()`.
      - This covers every operation that follows links: `read`, `read_range`, `write`, `append`, `list`, `glob`, and `copy`.
      - `contain_no_follow` (lines 166 to 183) calls `contain` on the parent, so a dangling link in the parent path of a no-follow operation is refused too. A final-component link is still addressed as a link.
      - The crate `README.md` (line 9) gains: content operations refuse a path that passes through a dangling symbolic link.
    - Grep removal. Delete:
      - `src/grep.rs` (`GrepQuery`, `GrepMatch`, and `GrepResults`, lines 9 to 44), and its `mod` and `pub use` in `src/lib.rs` (lines 19 and 32);
      - the `VfsAccess::grep` default method in `traits.rs` (lines 309 to 360), and the policy variant `Op::Grep` (line 426);
      - `Access::grep` in `handle.rs` (lines 2110 to 2138, which also name `Op::Grep` at lines 2119 and 2137);
      - the forwarding `grep` methods on `HandleAccess` (lines 2354 to 2355), `StoreMountSession` (lines 2452 to 2453), and `StoreScoped` (lines 2556 to 2557);
      - `RoutingAccess::grep` in `router.rs` (lines 318 to 331);
      - grep's tests in `traits.rs` (lines 506 to 507 and 723 to 780) and `detail.rs` (lines 480 to 512);
      - the grep mention in the `path.rs` doc (line 57).

      Neither `memory.rs` nor `host.rs` overrides grep. `ModePolicy`'s `is_mutation` has no `Grep` arm (`lib.rs`, lines 120 to 131). No lifecycle event exists for grep, no non-grep code calls a grep helper, and nothing in `crates/harness-internal/` uses grep. `StoreOp` has no grep variant, so the Lua store surface does not change.
    - `handle.rs`, claims gaps. The other side of each fix is an unordered claim from another task, compared as the neighboring checks do:
      - Nested subtrees. `check_subtree` (lines 838 to 916) looks up only `tables.subtrees.get(path)` (line 845). It also scans `tables.subtrees` and conflicts when `subtree_covers(path, other)` or `subtree_covers(other, path)` (`subtree_covers` at lines 1002 to 1008). Its callers need no change: `claim_subtree` (lines 780 to 791), `claim_rename` (lines 798 to 812), and the recursive branch of `Access::remove` (lines 1930 to 1933).
      - Subtree against parent listing. `record_subtree` (lines 921 to 930) never touches `tables.children`. `check_subtree` also conflicts with a listing of `parent_of(path)` (lines 968 to 978). `claim_list` (lines 680 to 709) also conflicts with a subtree claim whose root is a direct child of the listed directory, that is, `parent_of(subtree) == dir`. It must not use `subtree_covers(dir, subtree)`. That would also match a grandchild such as a recursive remove of `/a/b/c` against `list("/a")`, which does not change what `/a` lists, and would fail valid prompts with a false conflict. The existing check at lines 690 to 691 already covers subtrees that contain the listed directory.
      - Created ancestors. In `check_write` (lines 486 to 585), the `may_create` loop (lines 558 to 583) checks only `tables.paths`. It also checks `tables.children` for each created ancestor's parent, and `tables.patterns` through `pattern_matches_path` (lines 1043 to 1051) for each ancestor (`may_create` at lines 984 to 998). The leaf path already gets these checks (lines 523 to 541).
  - `promptforge-parser` (`crates/promptforge-internal/parser/src/`):
    - Optional-capability slots: a new check sits beside `check_distinct_aliases` in `contract.rs` (lines 379 to 392) and is invoked from `parse.rs` next to the existing alias check (lines 93 to 94), after the frontmatter is decoded. For each `ToolSlot::Exact(id)`, find a `CapabilityDecl` whose id equals `id.capability()` and whose `is_optional()` is true. If one exists, return `Error::parse(ParseErrorKind::Frontmatter, message)`. `capabilities` is a `Vec<CapabilityDecl>` with no uniqueness check today (`build.rs`, lines 76 to 78).
    - Duplicate capabilities: a second new check in the same place refuses a `capabilities:` list that names one capability id twice, with `ParseErrorKind::Frontmatter` and the message in Functional Specification. It runs before the optional-slot check, so a duplicate is reported as a duplicate.
      - An entry is either a bare string or a map with a required `ref` and optional `optional` and `config` (`CapabilityDeclVisitor` in `contract.rs`, lines 150 to 274). Both forms decode through `parse_capability_id` into a `GlobalName`.
      - Two entries are duplicates when their decoded ids are equal. `CapabilityId` and `GlobalName` equality is exact over the stored segments, with no normalization (`crates/promptforge-internal/types/src/capabilities.rs`, lines 32 to 66; `crates/promptforge-internal/types/src/names.rs`). Uppercase ids are already refused at parse, so a case-only difference cannot produce two distinct ids for one capability.
      - Entries that differ only in `optional` or `config` are still duplicates.
      - No prompt, agent program, guide example, or test fixture in the repository lists a capability twice. That was checked across `prompts/`, `local/`, `guide/src/`, and `crates/`, including `crates/harness-internal/sessions/agents/chat.md`, so nothing in the tree breaks. Nothing in `prompts/` or `local/` declares an optional capability. Guide examples and test fixtures that declare one bind no tool slot to it, so nothing in the tree breaks. The engine's run-time refusal in `crates/promptforge-internal/engine/src/execute/fill.rs` (lines 42 to 54) is then correct as written and stays.
    - Span documentation. The facade's `ParseError::span` doc (`crates/promptforge/src/lib.md`) and `error.rs` say the range is byte offsets in the source. It is relative to the document body after the frontmatter and a leading BOM, with CRLF normalized to LF. `line()` and `column()` locate the error in the original file (`with_prompt_context`, `error.rs` lines 104 to 114). Both docs say so. No code changes.
    - `build.rs`, `split_frontmatter` (line 281): the closer test `line.trim() == "---"` becomes `line.trim_end() == "---"`, so only a `---` at column 0 closes the frontmatter.
    - `build.rs`, `collect_headings` (lines 354 to 387): track nesting depth over `Tag::BlockQuote` and `Tag::Item` start and end events. Footnotes need no tracking: the parser runs `pulldown-cmark` with `Options::empty()` (`build.rs`, line 366), which leaves them off. A `Tag::Heading` inside either container is not a section boundary. Its text stays in the enclosing section's prose. No test, guide example, or in-tree prompt relies on the old behavior.
    - Structural error locations: the orphan heading and empty heading errors (`build.rs`, lines 483 to 500) are raised with `Error::parse`, which clears span and line (`error.rs`, lines 74 to 83). They are built the way the duplicate-sibling error is (`build.rs`, lines 532 to 541), carrying the heading's span, so `with_prompt_context` fills in line and column. The heading is already in hand at both raise sites (`build.rs`, lines 483 and 497), and its span is body-relative. The "`lua shared` outside H1" error (`parse.rs`, lines 134 to 146) carries the fence's range the same way: `shared_fences` already holds body byte offsets from `exact_shared_openings`. The unclosed-fence error (`fence.rs`, line 148) is left as it is: its callers hold the opening offset relative to the section's content, not the body, so a span there needs new offset plumbing (see Deferred and Out of Scope).
  - `promptforge-model-client` (`crates/promptforge-internal/model-client/src/`):
    - Validating constructors (`client/wire-canned.rs`, lines 16 to 47). `from_result(result: CompletionResult, model: impl Into<String>)` and `from_parts(id: impl Into<String>, name: impl Into<String>, arguments: Value)` return `Result<_, ClientError>`. They apply the rules in Functional Specification by calling the checks `normalize.rs` already uses, with no second copy. Where a check is written inline in `normalize.rs`, it is extracted into a small function that both call. `normalize.rs` does not call either constructor. The argument types stay as they are.
      - Every caller changes in the same change, because each stops compiling: the engine tests `crates/promptforge-internal/engine/src/execute/tests/serial_driver.rs` (lines 28 and 37 to 38) and `crates/promptforge-internal/engine/src/execute/run/effect-tests.rs` (line 92), and the compiled facade doc examples in `crates/promptforge/src/model.md` (line 81, and lines 328 to 333) and `crates/promptforge/src/effect.md` (lines 64 and 174). No harness, workshop, or facade integration test calls them.
      - The facade prose that introduces the two constructors (`model.md`, line 322) gains one sentence: they refuse the inputs listed in Functional Specification with a `ClientError`.
    - `client/stream.rs`, `SseScanner::next_data` (lines 60 to 74): lines end only on `\n` (line 62), and each call rescans from the buffer start. The scanner keeps the offset it already scanned without finding `\n` and resumes there, resetting it on drain. A CRLF split across two reads still works, because the `\r` stays buffered until the `\n` arrives (lines 63 to 65).
    - `client/stream.rs`, error envelope (lines 162 to 170): `chunk.get("error")` counts a present `null`. It becomes a present, non-null value.
    - `ClientError` (`error.rs`, lines 43 to 151) gains `#[non_exhaustive]`; the variant set and field visibility stay as they are.
      - Only an exhaustive `match` in another crate needs a new arm. The compiler names each one, so no manual search is needed. `matches!` and `if let` tests are unaffected.
      - Each new wildcard arm takes the outcome an unknown client failure should have. In the `RunErrorKind` mapping (`crates/promptforge-internal/engine/src/execute/error.rs`, around lines 104 to 110), that is `RunErrorKind::Completion`, which every `ClientError` already maps to. In the Lua error-kind mapping (`crates/promptforge-internal/engine/src/error.rs`, around lines 805 to 820 and 852), that is `ErrorKind::Internal`, which the harness-config variants already map to.
      - Other exhaustive matches the compiler reports take the outcome of their most general existing arm, and the choice is noted in the change description.
      - Matches inside `promptforge-model-client` (`model/error.rs`, lines 76 to 153) need no change.
  - `promptforge-types` (`crates/promptforge-internal/types/src/`):
    - `cancel.rs`: `Cancelled` (line 118) registers `cx.waker()` on each node in `poll` (lines 125 to 140) through `register` (lines 100 to 107). Each node holds a `Mutex<Vec<Waker>>`, deduplicated by `will_wake`. There is no `Drop`.
      - Removing by `will_wake` alone would be wrong. Two `Cancelled` futures polled by the same task hand in wakers that `will_wake` each other, so today they share one deduplicated entry. Dropping one would remove the entry the other still needs, which is a lost wakeup.
      - So each `Cancelled` takes a unique key when it is created, from an atomic counter. Each node's list stores `(key, Waker)` pairs, and deduplication is per key.
      - When a re-poll brings a waker that does not `will_wake` the stored one, the entry for that key is replaced.
      - A dropped `Cancelled` removes that key's entries under each node's mutex. The removal sits in the `Drop` of a private field type that `Cancelled` holds, not in an `impl Drop for Cancelled`, because the listing renders `Drop` impls (`crates/promptforge/public-api.txt`, line 384). The field keeps every auto trait the listing records for `Cancelled` (line 99).
      - When `cancel` (lines 176 to 187) already drained a node's list, removal is a no-op.
    - `ModelCatalog::from_validated` (`models.rs`, line 292) folds into `empty()` (line 299), its only caller.
  - `promptforge-engine` (`crates/promptforge-internal/engine/`):
    - Surface. Nothing outside the crate uses these except the bench and the `lib.md` doctests:
      - In `src/lib.rs`, `pub mod model` and `pub mod parser` (lines 8 and 9) become `pub(crate)`.
      - The root re-exports of `CompletionError`, `CompletionErrorKind`, `ParseError`, `ParseErrorKind`, `Prompt`, and `promptforge_version` (within lines 22 to 30) stop being public. Where engine code imports them through the crate root (`crate::Prompt` and similar), they become `pub(crate) use` instead of being deleted, so no internal import has to be rewritten.
      - `StoreOp` and `StoreOutcome` (`src/execute.rs`, line 71) become `pub(crate)`.
      - `benches/models_loop.rs` (lines 31 to 35) imports from `promptforge_model_client` and `promptforge_parser`, which are normal dependencies already.
      - The `lib.md` doctests (lines 12 and 26) use `promptforge::` paths through the doctest-only facade dev-dependency.
    - `Cargo.toml`: the `test-support` comment (lines 31 to 37) says the feature exists only for `benches/models_loop.rs` (`required-features`, line 65).
    - `entered` placeholder (`src/execute/scheduler/chain.rs`, line 77; `src/execute/scheduler/h1.rs`, line 64): initialize it from the section at the chain's start index, and fall back to the prompt title when the index is past the slice.
    - `start_live_h1` (`h1.rs`, lines 48 to 81) repeats the `Chain` literal that `start_chain` builds (`chain.rs`, lines 78 to 111). Both call one shared constructor.
    - `src/execute/scheduler/drive.rs` (line 205): `unwrap_or_else(|_| panic!(..))` converts an arena position. The loop iterates the arena's indices directly, so no conversion can fail.
    - `src/fanout.rs` holds only `parse_heading_address` and `resolve_sibling`. It is renamed `src/heading_address.rs`. References to update:
      - `src/lib.rs`, line 6;
      - `src/execute/engine.rs`, lines 13, 59, and 96;
      - `h1.rs`, lines 18 and 173;
      - `src/execute/scheduler/walk.rs`, lines 18 and 291.
    - `Environment::max_depth` is removed: the field, its default, its setter, and its `Debug` entry (`src/execute/environment.rs`, lines 34, 51, 61 to 62, and 144). Nothing reads it; the call depth cap is the `MAX_CALL_DEPTH` constant the scheduler checks (`src/execute/scheduler/tasks.rs`). Any test that calls the setter drops the call.
  - `build-xtask` (`crates/build-xtask/src/`):
    - The four `product-*.rs` files are `product-container-tests.rs`, `product-harness-tests.rs`, `product-test-support.rs`, and `product-tests.rs`. They are wired by `#[path]` from `product.rs` (lines 421 to 432) and fold into `product/`.
    - The three `api/listing-*.rs` files are `listing-compact.rs`, `listing-tests.rs`, and `listing-compact-tests.rs`. They are wired by `#[path]` from `api/listing.rs` (lines 101 to 106) and `api/listing-compact.rs` (lines 150 to 152), and fold into `api/listing/`.
    - Both folds follow the layout used for `crates/promptforge-internal/engine/src/execute/run/`.
  - The `promptforge-vfs` zero-dependency manifest test (`crates/promptforge-internal/vfs/src/lib.rs`, lines 172 to 211: `is_dependency_table` and `the_manifest_declares_no_dependencies`) also catches `[target.<cfg>.dev-dependencies]`, `[target.<cfg>.build-dependencies]`, and `[target.<cfg>.dependencies.<name>]` tables.
  - Family-wide comment sweep: audit tags such as `F3`, and history wording such as "legacy", "used to", "retired", and "formerly", are restated as present-tense constraints or deleted, per the comment rule in `AGENTS.md`. Known sites:
    - `crates/promptforge-internal/engine/src/error.rs`, lines 97, 948, and 974;
    - `crates/promptforge-internal/engine/src/test_support/recording.rs`, line 36;
    - `crates/promptforge-internal/engine/src/test_support/tokio_driver.rs`, line 36;
    - `crates/promptforge-internal/engine/src/execute/tests/scheduler/walk.rs`.
  - Structure. Every `.rs` file in the six crates ends at 500 lines or fewer, counted as `text.lines().count()`, including files the fixes push past 500. Files over the ceiling at `a05d5cbd`, with the lines before an inline test module in parentheses where one exists:
    - `promptforge-types`: `src/untrusted-inventory.rs`, 649 (503).
    - `promptforge-model-client`: `src/normalize.rs`, 1,181 (466).
    - `promptforge-parser`: `src/build.rs`, 572 (556); `src/tests.rs`, 1,441; `src/contract/tests.rs`, 562.
    - `promptforge-lua`: `src/vm.rs`, 1,457; `src/host.rs`, 679; `src/error-value.rs`, 543; `src/protocol/parse.rs`, 526; `src/tests.rs`, 3,653; `src/prelude-tests.rs`, 539.
    - `promptforge-engine`, source files: `src/error.rs`, 1,092 (860); `src/execute/scheduler/tasks.rs`, 760; `src/execute/scheduler.rs`, 563; `src/test_support/tokio_driver.rs`, 554.
    - `promptforge-engine`, test files under `src/execute/tests/`: `suite/exec_flow.rs`, 2,641; `scheduler/walk.rs`, 1,258; `scheduler/failures.rs`, 880; `scheduler/fanout.rs`, 854; `model_and_reply.rs`, 788; `tasks.rs`, 727; `scheduler/live_h1.rs`, 723; `happens_before.rs`, 670; `model_task_notices.rs`, 594; `tool_call_arm.rs`, 547; `context.rs`, 542; `debug_and_counts.rs`, 525; `scheduler/concurrency.rs`, 522.
    - `promptforge-engine`, other: `src/test_support/recording/forward-tests.rs`, 587.
    - `promptforge-vfs`: `src/handle.rs`, 3,825 (2,577); `src/host.rs`, 1,345 (706); `src/router.rs`, 977 (488); `src/memory.rs`, 886 (460); `src/traits.rs`, 810 (476); `src/detail.rs`, 620 (110).
  - Split method:
    - A file that is over only because of its inline test module moves that module to a kebab-named `-tests.rs` sibling wired with `#[path]`, the pattern the crates already use. This covers `normalize.rs`, `router.rs`, `memory.rs`, `traits.rs`, and `detail.rs`. A moved test module that is itself over 500 lines splits again by topic.
    - A source file splits along its existing seams. For `handle.rs`, those are the scope, the claims ledger, the handle, the store view, and the forwarding wrappers.
    - A test file splits by the topic groups it already contains.
    - Moved code stays byte-identical apart from `use` lines, module wiring, and visibility, so each split's diff is reviewable as a move. Moved items take the narrowest visibility that compiles (`pub(super)` or `pub(crate)`), no public path changes, and no test moves between crates.
  - Enforcement. Each crate's `src/lib.rs` gains a `//! ## Invariants` section. That is the literal `has_marker` looks for, in `src/lib.rs` or `src/main.rs` (`tidy.rs`, lines 56 and 295 to 298). Its bullets follow the `new-crate` template (`crates/build-xtask/src/new_crate.rs`, lines 66 to 76):
    - a line naming what the crate may depend on, matching the dependency lists in `crates/promptforge-internal/README.md`. The template's Tier wording names workshop tiers, which do not apply here, so it is left out;
    - "Every file in this crate stays under 500 lines; split first, then edit."

    The engine's crate docs come from `#![doc = include_str!("lib.md")]`, so its marker goes in `src/lib.rs` as `//!` lines beside that attribute. The other five crates already use `//!` docs. For a crate outside the `workshop-*` and `harness-*` families, the marker switches on only `file_ceiling_violations` and `lint_inheritance_violations` (`tidy.rs`, lines 51 to 74 and 177 to 367). All six crates already inherit `[lints] workspace = true`.
- File and public API changes:
  - Facade listing (`crates/promptforge/public-api.txt`):
    - the new signatures of `Completion::from_result` and `ToolCall::from_parts`;
    - `#[non_exhaustive]` on `ClientError`;
    - removal of the grep items: the three grep types and their members (lines 74 to 76, 185 to 187, 310 to 312, and 1290 to 1300), `Access::grep` (line 645), `VfsAccess::grep` (line 683), and `Op::Grep` (line 1310);
    - removal of `Environment::max_depth` (line 420).
  - Facade re-exports (`crates/promptforge/src/lib.rs`): `GrepMatch`, `GrepQuery`, and `GrepResults` (lines 149 to 151) are removed. `Access` (line 143) and `VfsAccess` (line 166) stay.
  - Facade docs:
    - the `Completion::from_result` and `ToolCall::from_parts` doc examples and their introducing sentence in `crates/promptforge/src/model.md`, and the `from_result` calls in `crates/promptforge/src/effect.md`;
    - the `ParseError::span` doc in `crates/promptforge/src/lib.md`;
    - the `max_depth` text in `lib.md` (line 398);
    - in `crates/promptforge/src/vfs.md`: the `GrepQuery`, `GrepResults`, and `GrepMatch` sections (from line 665), the `Access::grep` and `VfsAccess::grep` entries (lines 558 and 881), and the grep mentions at lines 379, 667, and 729.
  - Guide:
    - `guide/src/language/12-tools.md`, in "Declaring capabilities" and "Tool slots and Tool objects": a tool slot requires its capability, and an optional capability cannot back one.
    - `guide/src/language/02-file-structure.md`, "Frontmatter rules and errors": gains both new refusals, the optional-capability slot and the duplicate capability.
    - `guide/src/language/12-tools.md`, "Declaring capabilities": also states that each capability is declared once.
    - Regenerate the combined guide with `cargo run --locked -q -p build-user-guide`.
  - Crate docs: the six internal crates' `src/lib.rs` gain the marker section, and `crates/promptforge-internal/vfs/README.md` gains the dangling-link sentence.
  - Renamed: `crates/promptforge-internal/engine/src/fanout.rs` becomes `src/heading_address.rs`.
- Data, persistence, failure, security, and privacy constraints:
  - Run-log, replay, and wire JSON formats do not change.
  - The reserved-name list and its drift test are unchanged.
  - `var` snapshots and `var_to_json` omit removed keys, as today.

</implementation-contract>
<verification-contract>

## Testing Plan

Each cancellation and sandbox fix gets a test that drives the exact construct that failed, and each claims gap gets a two-task test in both orders. The constructor changes are covered in the model client and by the engine tests and facade doc examples that build completions. The splits are proven behavior-free by an unchanged test count and an unchanged listing, after which the existing ceiling check guards all six crates. The exit gates are the repository's full set.

- Unit:
  - Lua cancellation, in `crates/promptforge-internal/lua/src/`:
    - A looping handler around a looping body, `xpcall(function() while true do end end, function() while true do end end)`, ends as interrupted under a set cancel flag.
    - A non-looping handler still receives ordinary failures.
    - `setmetatable({}, { __gc = f })` is refused, so no author finalizer can be installed.
    - `setmetatable(_G, { __mode = "v" })` is refused.
    - `setmetatable(_G, { __index = f })` still works.
    - The existing cancellation tests around line 2285 of `crates/promptforge-internal/lua/src/tests.rs` keep passing.
  - Lua cleanup:
    - `var.k = nil` removes `k` from `var` and from its snapshot.
    - `the_models_namespace_has_no_bind` (`crates/promptforge-internal/lua/src/models-tests.rs`) is updated for the removed Rust placeholder.
    - Every engine test that calls `models.infer` passes unchanged. That is about 130 call sites across 23 test files, including `crates/promptforge-internal/engine/src/execute/tests/live_infer.rs` and `tests/scheduler/live_h1.rs`, which cover sections and the live H1 pass. This is the guard that removing the placeholder does not remove `models.infer`.
  - VFS host, skipping with a logged reason where the host lacks symlink privilege:
    - A dangling link inside the root refuses `append`, `write`, `read`, and `list`, and no file appears at the link's target.
    - `remove` and `exists` still act on the dangling link itself.
    - The Windows escape test (`crates/promptforge-internal/vfs/src/host.rs`, lines 875 to 907) logs a skip instead of passing silently, and also covers `append`.
    - The test near line 1034 of the same file that is named for a `..` clamp is renamed for what it asserts.
  - VFS claims, in the claims tests of `crates/promptforge-internal/vfs/src/handle.rs`, each with two tasks not ordered by a join, in both orders:
    - A recursive remove of `/a` against a recursive remove or rename of `/a/b`.
    - `list("/a")` against `remove("/a/b", true)`.
    - `list("/a")` against `remove("/a/b/c", true)` does not conflict, because the listing of `/a` is unchanged.
    - A write that creates `/a/b/c` against `list("/a")` and against `glob("/a/*")`.
    - The same pairs with a join between the tasks do not conflict.
  - Parser, in `crates/promptforge-internal/parser/src/` (`contract/tests.rs` for the frontmatter checks, `tests.rs` for the rest):
    - The optional-capability slot refusal, with its exact message.
    - A prompt that declares an optional capability without a slot still parses.
    - An indented `---` inside a YAML block scalar does not close the frontmatter.
    - A heading inside a block quote or a list item is not a section, and its text stays in the enclosing section's prose.
    - The orphan heading, empty heading, and "`lua shared` outside H1" errors report a line.
    - A capability listed twice is refused with the duplicate message: both entries plain, both optional, one of each, and two map entries that differ only in `config`.
    - A list of distinct capabilities still parses.
  - Model client, in `crates/promptforge-internal/model-client/src/` (the stream tests in `client/stream-tests.rs`):
    - `from_parts` refuses a blank id, a blank name, and non-object arguments.
    - `from_result` refuses an empty tool-call batch and duplicate call ids, and accepts a text result.
    - A `data:` line delivered in many one-byte reads is returned whole, and only once, when its newline arrives. The resume offset is exercised by this test. There is no timing or byte-count assertion, so no test-only instrumentation is added.
    - A chunk with `"error": null` parses as an ordinary chunk.
    - `error_envelope_fails_the_stream_with_the_escaped_message` keeps passing.
  - Types, in `crates/promptforge-internal/types/src/`:
    - A `Cancelled` future dropped before cancel leaves no waker on any ancestor.
    - Two `Cancelled` futures on one handle, polled from the same task: dropping one leaves the other woken by a later cancel.
    - A future re-polled with a different waker is woken through the new waker only.
    - Every `Event` variant round-trips. `crates/promptforge-internal/types/src/event-tests.rs` (lines 47 to 143) covers nine today.
    - An exhaustive match in the crate's own tests maps every payload-free `Event` variant to its lifecycle constant, so a new variant fails to compile until it is mapped. `crates/promptforge-internal/types/src/event-lifecycle.rs` (lines 41 to 122) covers only the constants its list names.
  - Engine:
    - The `entered` placeholder names the start section for a chain that starts past index 0.
    - The conflict line of `Requirements::notice` has its wording and position pinned (`crates/promptforge-internal/engine/src/execute/requirements-tests.rs`).
    - A capability prelude global that collides with `tools`, `store`, or `models` is refused (`crates/promptforge-internal/lua/src/prelude-tests.rs`).
  - build-xtask: fixture manifests using each newly caught dependency table form are refused by the `promptforge-vfs` manifest test.
- Integration and end-to-end:
  - The engine tests that build completions move to the validating constructors: `crates/promptforge-internal/engine/src/execute/tests/serial_driver.rs` (lines 28 and 37 to 38) and `crates/promptforge-internal/engine/src/execute/run/effect-tests.rs` (line 92).
  - The facade doc examples that build completions compile and pass against the validating constructors: `cargo test --locked --doc -p promptforge --all-features` covers `crates/promptforge/src/model.md` and `effect.md`.
  - The reserved-name drift test keeps passing.
- Regression, security, and performance:
  - Splits: each split step reads the `test-count` field of `cargo nextest list --locked -p <crate> --all-features --message-format json` before its first edit and after its last, for every crate it splits, and the count after is no lower. Splits never move a test between crates, so the per-crate count is exact. Each split step also leaves `cargo +nightly-2026-09-05 xtask api --check` passing with no listing change.
  - Enforcement: `participating_crates_respect_the_file_line_ceiling` (`crates/build-xtask/src/tidy-tests.rs`, line 22) passes with all six crates participating.
- Exit criteria:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`
  - `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
  - `cargo check -p gateway --no-default-features`
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, including the engine's private-item docs job
  - `cargo test -p build-xtask`
  - `cargo +nightly-2026-09-05 xtask api --check` (the nightly pinned in `crates/build-xtask/src/api/toolchain.rs`), passing against the listing as re-blessed by each API-changing change with `cargo +nightly-2026-09-05 xtask api --bless`, with the cumulative diff matching File and public API changes
  - `cargo deny check`, which CI runs (`.github/workflows/ci.yml`, lines 340 to 341)

</verification-contract>
<decision-record>

## Decision Record

Every change is the smallest one that satisfies its finding. Where a finding's fix would have committed the code to a design nobody chose, the plan either shrank it to a documentation fix or made it an open question. Several findings turned out to be documented behavior, or to have consumers or conditions that changed the right fix, and those are recorded here with their evidence. The file splits and their enforcement belong to this batch, with each crate turning enforcement on as soon as it is clean.

- Decisions:
  - Scope: every open defect of Critical or Important severity in the six crates, plus the low-severity items in the same code. The resulting list is exactly the set of changes under Technical Design. User's choice: "Every open Critical and Important finding (the 3 new ones, optional capability, VFS claim gaps, Lua var/proxy/infer/store, model-client constructors and ClientError, waker leak, parser spans, engine surface and test-support), plus the low items that sit in the same code (Recommended)".
  - Smallest change per finding, with anything larger put to the user. User's words: "make sure we are not building out too much for example for the frontmatter compliance, without direct input from me. I dont want a decision like \"require the capability\" to unexpectedly turn into writing a lot of code that assumes a lot of things."
  - A tool slot may not name an optional capability, and it is refused at parse. User's choice: "Forbid it: a tool slot may not name an optional capability, refused at parse with a clear error". The whole implementation is one check beside `check_distinct_aliases` and its guide text.
  - A capability declared more than once in frontmatter is a hard parse error, whatever its `optional` flags. User's words: "mentioning a capability twice in the front matter should be a hard fail error".
  - The small, uncontroversial leftovers join the batch. User's words: "whatever is easy and uncontroversial to fix, yuo should fix".
  - The file splits and ceiling enforcement join this batch. User's words: "I want it all, in the plan we just made ... break the files up and turn on enforcement."
  - Each crate gains its marker in the change that brings its last file under the ceiling, so the check never fails the build on arrival.
  - Splits are pure moves, kept apart from fix changes, and a file's fixes land before its split. The "split first, then edit" rule in `AGENTS.md` binds only crates that already carry the marker, and none of the six does until its split is done.
  - The split work runs as nine steps, each small enough for one sub-agent to read every file it cuts and for one reviewer to read its diff: no split step covers much more than 4,700 lines. The engine alone holds 14,827 oversized lines, so one step per crate would not fit. The test-count check runs per crate inside each split step, so no step depends on a number recorded by another. User's choice: "Apply all of them, including the finer split steps and the copied survey (Recommended)".
  - The Project Survey is carried over from the previous plan in this repository (`vibe/2026-09-28-4-internal-crates-critical-fixes.md`), with the facts that plan's run changed brought up to date, so the run does not re-survey.
  - Cancellation: the `xpcall` handler is skipped under cancellation, and `__gc` is refused outright. Lua 5.5.0 runs both with hooks disabled, so no cancel check can interrupt them. `__close` needs nothing, because hooks are restored before it runs.
  - Dangling links are refused rather than resolved and contained. It is the smallest change, and it is consistent with the README's rule that containment denies a link that resolves outside the root.
  - Parser spans get a documentation fix only. Production hosts use `line()` and `column()`, which already locate errors in the original file (`crates/workshop/server/src/error.rs` uses `line()`), and nothing outside the parser's tests reads `span()`.
  - The store closures get a documentation fix only. They are not duplicates of `run_store_op`: they add lifecycle reporting, conflict recording, and error wrapping.
  - `from_result` and `from_parts` keep their argument types and gain only validation and a `Result`. Text results are not validated.
  - The two wire-name rules stay as they are: `ToolCatalog::new` keeps its documented rule, and `tool_schema_new` keeps its stricter one. User's choice: "Drop it from this batch and record it as a rejected alternative; Steps 1 and 2 lose that item (Recommended)".
  - The id error messages stay as they are. User's choice: "Drop it from this batch and record it as a rejected alternative; frontmatter errors already name the id, and hosts calling ModelId::new hold their inputs (Recommended)".
  - `var.k = nil` is tested before it is changed, because the code path traced shows the key is already removed.
  - Glob across nested mounts and Windows path aliasing on `HostBackend` are deferred. No production host nests mounts or mounts `HostBackend`. User's choice: "Defer both, with the evidence recorded; revisit when a host nests mounts or mounts HostBackend on Windows (Recommended)".
  - Grep and `Environment::max_depth` are removed from the public surface, because both are dead. User's choice: "Remove both (Recommended)".
  - `ClientError` gains `#[non_exhaustive]` and nothing else. User's choice: "Add #[non_exhaustive] only; the engine and harness add wildcard arms where they match (Recommended)".
  - `Cancelled` deregisters through the `Drop` of a private field it holds, not through an `impl Drop for Cancelled`. `Cancelled` is on the facade as `promptforge::cancel::Cancelled` (`crates/promptforge/public-api.txt`, lines 99, 224, and 379 to 380), and the listing renders `Drop` impls (line 384, for `promptforge::vfs::Access`). An impl on `Cancelled` itself would therefore add a public trait impl and a listing line that File and public API changes does not name, in a change the plan says leaves the listing alone. The private field's `Drop` performs the same removal with no public change. Settled during decomposition on 2026-09-28, not chosen by the user.
- Rejected alternatives:
  - `__pairs` and `__len` on guarded `var`, frozen `argv`, and sealed tables, and fanout through them. Reason: the guide documents empty iteration as intended and says to use key access or `ipairs` (`guide/src/language/05-lua-environment.md`, lines 434 to 443; `06-arguments.md`, lines 420 to 462). Revisit: if the user wants iteration over guarded tables, as a documented contract change.
  - Merging the Lua store closures into one implementation. Reason: they differ in behavior. Revisit: if the shared-library load path moves onto the yield shims.
  - A body-to-source offset map for `ParseError::span`. Reason: no consumer. Revisit: when a host needs byte offsets into the original source.
  - Collapsing the five harness-config `ClientError` variants. Reason: `CompletionErrorKind` distinguishes `Disabled` from `Config` (`crates/promptforge-internal/model-client/src/model/error.rs`, lines 81 to 86), and each variant has its own message, so users would lose information. Revisit: never.
  - Moving `ModelSetLock` out of `ClientError` or reclassifying it as internal. Reason: `CompletionErrorKind` has no internal kind (`model/error.rs`, lines 23 to 35), so it would add a public kind. Revisit: when a kind for internal faults is designed.
  - SSE CR-only line endings and multi-line `data:` joining. Reason: no backend or fixture uses either. Joining would also require event dispatch on blank lines, which the scanner does not implement, and would change when payloads are delivered. Revisit: when a backend sends either.
  - Resolving a dangling link's target and containing it. Reason: more code for a case no production host has. Revisit: if a host needs appends through in-root dangling links.
  - A parameters-schema check in `ToolCatalog::new`. Reason: it needs a new public `ToolCatalogError` variant, and `tool_schema_new` already refuses a non-object schema when the tool is advertised. Revisit: if schema errors need to surface at assembly, as a user decision on the new variant.
  - One wire-name rule shared by `ToolCatalog::new` and `tool_schema_new`. Reason: `validate_identifier` (`crates/promptforge-internal/types/src/tools/ids.rs`, lines 194 to 216) has no caller besides the catalog's wire-name check (`tools/registry.rs`, line 62), so sharing the rule would delete it and `ToolIdError::reason` and leave `ToolIdErrorKind::Separator` produced by nothing. It would also make the catalog refuse wire names the facade documents as accepted ("Uppercase and other printable characters are accepted in a wire name", `crates/promptforge/src/tools.md`, line 373), a host contract change. A name the catalog accepts but `tool_schema_new` refuses still fails when the tool is advertised. Revisit: if hosts need that failure at catalog assembly, as a documented contract change.
  - Naming the rejected value in `ModelIdError`, `GlobalNameError`, and `CapabilityIdError`. Reason: the facade documents their `Display` text as a fixed prefix and reason (`crates/promptforge/src/model.md`, line 431; `crates/promptforge/src/capabilities.md`, lines 317 and 361), four facade doc examples assert it exactly (`model.md`, line 124; `capabilities.md`, lines 40, 127, and 131), and the guide quotes the parser message that wraps it (`guide/src/language/12-tools.md`, line 180). That parser message already names the id, so it would print it twice. Hosts calling `ModelId::new` hold their inputs. Revisit: if a host reports an id error it cannot trace to its input.
  - Making `Backend`'s body private behind a bounding constructor. Reason: the harness already caps and escapes before building it (`crates/harness-internal/models/src/transport.rs`, lines 352 to 363), and the user chose `#[non_exhaustive]` only. Revisit: when an outside transport exists.
  - Fixing glob across nested mounts, or Windows aliasing, now. Reason: the user deferred both. Revisit: when a host nests mounts or mounts `HostBackend` on Windows.
  - Leaving the alias unbound with a warning for an optional-capability slot, or keeping the run-time refusal with better wording. Reason: the user chose a parse-time refusal. Revisit: if prompts need optional tools, as a new frontmatter form.
  - Removing the engine's `test-support` feature and bench. Reason: the bench is the only benchmark of the model loop. Revisit: if the bench moves to the facade.
  - A separate later plan for the splits, adding all six markers first, or splitting before fixing. Reason: the user wants the splits in this batch; markers first would fail the build at once; and fixes mixed into moved code are hard to review. Revisit: never.
  - `impl Drop for Cancelled`, with a re-bless in the types change. Reason: it adds a public trait impl the user has not approved and a listing change outside File and public API changes, for no behavior the private field lacks. Revisit: if the user wants the impl public, add it to File and public API changes and re-bless in the types change.
- Assumptions, risks, and notes:
  - Line numbers are at `a05d5cbd`. Fixes shift them, and so do commits that land on `master` after `a05d5cbd`, so the implementer re-resolves each by the function name given. Before starting, run `git log a05d5cbd..HEAD -- crates/promptforge-internal crates/promptforge crates/build-xtask`. If it lists commits, re-check each premise this plan cites in the files they touch.
  - The claims listing check is deliberately narrow: a listing conflicts only with subtrees whose root is a direct child of the listed directory, or that contain it. A broader match would report races that change nothing the listing returns.
  - The `Cancelled` fix keys registrations per future. Deduplicating by `will_wake` alone makes futures polled by the same task share one entry, and dropping one would silence the other.
  - The claims fixes can turn a previously passing concurrent prompt into a claims conflict. That is the intended outcome for a real race.
  - `crates/promptforge-internal/vfs/src/handle.rs` is the riskiest split: the claims ledger's internals must stay private to the VFS while becoming visible across the new modules.
  - Three file-symlink VFS tests skip on Windows hosts without symlink privilege. Junctions cover the directory case there.
  - `HostBackend` has no production caller, so the dangling-link fix protects outside hosts and future ones.
  - Guard-nonce determinism needs no new test: `a_seeded_nonce_is_a_function_of_its_seed_alone` (`crates/promptforge-internal/types/src/untrusted-tests.rs`, line 32) and `two_runs_with_the_same_seed_and_started_at_produce_identical_nonces_and_sys_when` (`crates/promptforge-internal/engine/src/execute/tests/run_inputs.rs`, line 47) already pin it.
  - `#[non_exhaustive]` on `ClientError` breaks no facade doc example: `crates/promptforge/src/transport.md` tests variants only with `matches!`.

### Deferred and Out of Scope

- Deferred: glob ignoring nested mounts, and Windows path aliasing on `HostBackend` (case, trailing dot or space, `:` streams, `C:` segments). Revisit when a host nests mounts or mounts `HostBackend` on Windows.
  - No production host nests mounts (`crates/harness-internal/sessions/src/runtime.rs`, lines 49 to 59), and none mounts `HostBackend`.
  - Store paths already refuse trailing dots, trailing spaces, and Windows device names on every platform (`crates/promptforge-internal/vfs/src/handle.rs`, lines 1246 to 1307).
- Deferred: a long-running single C library call, such as a pathological Lua string pattern, `table.sort` over a huge array, or `string.rep`, cannot be interrupted by the instruction hook, so cancellation waits for it to return. Revisit as its own design, for example pattern limits.
- Deferred: tool-call `id` and `name` fragments that repeat in full on every SSE delta (`crates/promptforge-internal/model-client/src/client/stream.rs`, around lines 275 to 300). Revisit when a backend is seen doing it.
- Deferred: waking a dropped live timer's waiter, and reporting answers that arrive after the run is decided (`crates/promptforge-internal/engine/src/execute/scheduler/apply.rs`, `scheduler/drive.rs`). The timer rule is documented as deliberate.
- Deferred: the plan-mode `.md` suffix rule in `crates/promptforge-internal/vfs/src/lib.rs`, which needs the policy to see file types.
- Deferred: a location on the unclosed-fence error (`crates/promptforge-internal/parser/src/fence.rs`, line 148). Its callers (`split_h1`, lines 54 and 61; `split_section_blocks`, lines 266 and 287) hold the opening offset relative to the section's content, so a body-relative span needs the section's body offset threaded through. Revisit when a host reports a fence error it cannot locate.
- Deferred: the `unwrap_or_else(|| panic!(..))` sites in `crates/promptforge-internal/vfs/src/handle.rs` and `router.rs`. They guard invariants that hold, and replacing them needs a decision on an internal `VfsError` kind.
- Deferred: API-shape choices without a settled answer:
  - vLLM metrics are reachable only through a detail function.
  - `types::detail::model_id_from_validated` is kept for other crates' tests.
  - `overlay`, `mount`, and `store` panic on bad input instead of returning errors.
  - A remove of an unmounted path reports success.
  - An uncaught `TaskCancelled` classifies as a Lua error.
- Out of scope: the `promptforge` facade crate's marker, which the repository leaves off by design (`AGENTS.md`, line 83).
- Out of scope: any change to run-log or replay formats.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p <package>`. Plain `cargo build` builds only the default member, `crates/gateway/app` (package `gateway`). The six internal crates (`promptforge-engine`, `promptforge-lua`, `promptforge-vfs`, `promptforge-model-client`, `promptforge-types`, `promptforge-parser`) and the `promptforge` facade build with no UI or native prerequisites. Crates whose build scripts bundle a UI into `OUT_DIR` (`workshop-server`, `gateway-config-ui`, and their dependents, which the workspace-wide runs include) need `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first. Before any `-p workshop` build, CI builds `cargo build --locked -p gateway --no-default-features` and stages it with `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe` (undo with `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`). The clippy and full-suite runs build every member they check, so a separate workspace build adds nothing beside them, and a standalone `cargo check --workspace` never runs beside clippy. `.cargo/config.toml` aliases `cargo xtask` to `run -p build-xtask --` and `cargo workshop` to `run -p build-workshop --`, and links Windows builds with `rust-lld` and the static CRT.
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <filter>`, where `<filter>` is one or more test-name or module-path substrings (nextest runs a test matching any of them), such as `host::tests`, `model_task_notices`, or `scheduler::concurrency`. The internal crates keep every test inside `src/` as unit-test modules and have no integration target; the `promptforge` facade's integration target is `--test suite`. Drop `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. Nextest skips doctests, so a doc example needs `cargo test --locked --doc -p <package> --all-features`. `.config/nextest.toml` marks a test slow at 60 seconds and terminates it after three periods, 180 seconds. A crate's test count is the `test-count` field of `cargo nextest list --locked -p <package> --all-features --message-format json`.
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features`, then `cargo test --locked --doc -p <package> --all-features` (the workshop trio without `--all-features`). The structural harness alone: `cargo test -p build-xtask`; its nightly-only fixtures: `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`. CI adds `cargo nextest run --locked -p workshop-workspace --all-features`, `cargo nextest run --locked -p workshop-server --features headless`, the gateway process-ownership race tests, and the UI `npm test` runs. The workspace run includes `build-xtask`, the structural harness.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`, plus the headless gate `cargo check -p gateway --no-default-features`, `cargo deny check`, and `cargo audit` (CI's `supply-chain` job; `cargo-deny` 0.20.2 and `cargo-audit` 0.22.2 are installed locally). Per-package pattern for scoped runs: `cargo clippy --locked -p <package> --all-targets --all-features -- -D warnings`. The UI typechecks (`npm run typecheck --workspaces --if-present` from `crates/workshop`, `npm run typecheck` from `crates/gateway/config-ui/ui`) apply only to UI changes. `.githooks/` holds a pre-commit hook (the formatter check) and a pre-push hook (the headless check, the workspace clippy, `cargo deny check`); neither is installed in this clone, since `core.hooksPath` is unset.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml` sets `style_edition = "2024"`). No UI formatter or JS linter is configured.
- Docs command: with `RUSTDOCFLAGS` set to `-D warnings` (PowerShell: `$env:RUSTDOCFLAGS='-D warnings'`), run `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then the facades alone with default features, `cargo doc -p promptforge --no-deps` and `cargo doc -p harness --no-deps`, then the engine's private items, `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`. These are CI's `docs` job; CI's `check-workshop` job also builds `cargo doc --locked --no-deps -p workshop-server --document-private-items`. Per-package pattern: `cargo doc --locked --no-deps --all-features -p <package>` under the same flag. The facade surface check is `cargo +nightly-2026-09-05 xtask api --check` (the nightly pinned in `crates/build-xtask/src/api/toolchain.rs`, installed locally), compared against the committed `crates/promptforge/public-api.txt`; `cargo +nightly-2026-09-05 xtask api --bless` rewrites it. User guide: `cargo xtask site --books-only`; the combined guide regenerates with `cargo run --locked -q -p build-user-guide`.
- Test placement and naming conventions:
  - Unit tests sit in a sibling file `<module>-tests.rs`, wired as `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;` (for example `engine/src/execute/requirements-tests.rs`, `lua/src/prelude-tests.rs`, `types/src/event-tests.rs`, `model-client/src/client/stream-tests.rs`). Small modules keep an inline `#[cfg(test)] mod tests {}` at the bottom (`engine/src/execute/config-limits.rs`, `parser/src/list.rs`), and `promptforge-vfs` uses inline blocks throughout (`vfs/src/host.rs` from line 707, whose helper `make_dir_link` makes a junction on Windows through `mklink /J` and a symlink on Unix). VFS tests return `Result<(), VfsError>`.
  - `promptforge-engine`'s behavior suite is `src/execute/tests/`, declared by `execute/tests.rs`: one file per area (`model_task_notices.rs`, `model_tasks.rs`, `local_tools.rs`, `models_loop_compactors.rs`, `tool_loop.rs`, `run_termination.rs`, and others) plus `scheduler/` (`concurrency.rs`, `walk.rs`, `failures.rs`, `fanout.rs`, `live_h1.rs`, `store_gate.rs`) and `suite/`. These tests drive whole runs through the `test_support` drivers against canned or scripted models. `cancel_during_in_flight_tool_call_returns_promptly` (`tool_loop.rs`, line 470) is the mid-run cancel pattern: a multi-thread tokio test, `TokioDriver::cancel_handle`, a cancel after 100 ms, and an `Err(crate::Error::Interrupted)` assertion within 5 seconds. `engine/tests/prompts/` holds fixture prompts, not a test target.
  - `promptforge-lua` keeps one large `src/tests.rs` beside its `<module>-tests.rs` siblings, `protocol/tests/`, and `tools/tests.rs`; shared helpers sit in `tests-recording.rs`. Its cancellation tests, `long_running_lua_block_cancels_cooperatively` and `a_pre_cancelled_run_aborts_a_tight_loop_promptly`, are in `tests.rs`. `prelude-tests.rs`'s `section_vm_with_var` builds a section VM with the scheduler control globals and coroutine shims installed, the setup under which the shim's `pcall` and `xpcall` replacements are live.
  - Integration targets: `tests/suite/` in the `promptforge` and `harness` facades, `tests/it/` in harness and workshop crates. Each `main.rs` opens with `#![expect(clippy::expect_used, clippy::unwrap_used, reason = ...)]`.
  - Shared fixtures live in `test_support` modules behind a `test-support` feature (engine, lua, parser, runner, sessions) or a `test-fixtures` feature (gateway and workshop crates).
  - Test functions are snake_case sentences stating the behavior, such as `a_rooted_backend_rejects_links_that_escape_the_mount_root`. `clippy.toml` allows `unwrap` and `expect` in tests; the workspace lints deny both elsewhere.
  - Benches: `engine/benches/models_loop.rs` and `lua/benches/surface.rs`, both requiring `test-support`. Both are under 500 lines.
- Directory map:
  - `crates/`: every Rust crate and UI package. Its root is the public layer: the `promptforge` facade (`src/`, `tests/suite/`, `public-api.txt`), the `harness` facade, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, `shared-ui` (TypeScript and CSS, not a crate), the `build-*` tooling crates, and `workspace-hack`.
  - `crates/promptforge-internal/`: a manifestless container holding `engine` (`src/execute/` with `run/`, `scheduler/`, and `tests/`, `src/test_support/` with `recording/`, `src/lua/`, `src/model/`, `src/lib.md` as the crate doc, `benches/`, `tests/prompts/`), `lua` (the section VM, the Lua shims `__impl_coro.lua`, `__impl_fanout.lua`, `__impl_messages.lua`, `__impl_store.lua`, `__impl_tasks.lua`, plus `protocol/` and `tools/`), `vfs` (a flat `src/`, std only), `model-client` (`client/`, `model/`), `types` (`tools/`), and `parser` (`contract/`). Each crate has `AGENTS.md`, `Cargo.toml`, and `README.md`; the container's `README.md` describes the six.
  - `crates/harness-internal/`, `crates/workshop/`, and `crates/gateway/`: the other family containers (the harness runner, models, capabilities, log, sessions, and web crates; the Workshop desktop app, server, and UI npm workspaces; the gateway app and its subsystems, including the nested `stt/`).
  - `guide/`: user guide chapter sources (`src/language/` among them), mdBook books, and the combined `guide/promptforge-language-guide.md`. `prompts/`: sample prompts. `tools/`: Node and Python maintenance scripts.
  - `vibe/`: `archdoc.md`, dated plan and run records (`YYYY-MM-DD-N-slug.md`), reference notes, and a gitignored `scratch/`.
  - `.github/workflows/ci.yml` (jobs `fmt`, `clippy`, `test`, `docs`, `check-workshop`, `check-workshop-linux`, `ui`, `supply-chain`, `api-surface`, and the aggregate `ci-green`), `.githooks/`, `.config/nextest.toml`, `.cargo/config.toml`, and at the root `Cargo.toml` (an explicit container member list; `default-members` is the gateway app), `rust-toolchain.toml` (stable), `clippy.toml`, `rustfmt.toml`, and `deny.toml`.
- Component boundaries:
  - `promptforge` is a facade of single-item re-exports over `crates/promptforge-internal/*` and the only promptforge crate that code outside the family may name. `promptforge-*` crates depend on no gateway, workshop, or harness crate; an internal crate may list `promptforge` as a dev-dependency only so its doc examples compile.
  - Inside the container dependencies run one way: `promptforge-vfs` depends on nothing, which its manifest test enforces; `promptforge-types` depends on no sibling; `promptforge-model-client` on types; `promptforge-lua` on model-client, types, and vfs; `promptforge-parser` on lua and types; `promptforge-engine` on all five. The engine performs no I/O and names tokio only behind `test-support`.
  - `harness` fronts `crates/harness-internal/*`, whose crates depend only on `promptforge`. Workshop crates may name `harness`, `promptforge`, the gateway public pair, and `shared-*`. Gateway private crates depend on no promptforge, harness, or workshop crate. `shared-*` crates depend on no product crate.
  - `cargo test -p build-xtask` enforces the topology, the `## Invariants` markers in workshop-* and harness-* crates, lint inheritance, and the 500-line ceiling in marker crates. None of the six internal crates carries the marker until this plan's split steps add it. `cargo xtask api --check` enforces the facade surface.
- Conventions summary:
  - Rust 2024 edition on the stable toolchain. Workspace lints every member inherits: `unsafe_code = "forbid"`; `missing_docs`, `missing_debug_implementations`, and `unreachable_pub` warn; clippy `all` and `pedantic` denied; `unwrap_used` and `expect_used` denied; broken and private intra-doc links denied, so removing an item a doc comment links to fails the docs build.
  - Source directories are flat: one or two files beside a parent module are `foo-bar.rs` wired with `#[path = "foo-bar.rs"] mod bar;`; at three they become a `foo/` subdirectory in standard layout with no path attributes, and they flatten back below three. `engine/src/execute/run/` and `engine/src/test_support/recording/` show the folded form. `crates/build-xtask/src/` still holds four `product-*` siblings and `api/` three `listing-*` siblings, which this plan folds.
  - Behavior changes ship with tests in the same change. No new structural enforcement without explicit user approval.
  - Error and status messages are written for a model reader: concise, self-contained, naming required versus actual.
  - Comments explain only a non-obvious constraint, ordering requirement, or workaround; every workaround cites its upstream issue URL.
  - JSON that reaches the run log round-trips exactly: sorted keys, finite numbers, `float_roundtrip`, never `preserve_order`.
  - Cargo features gate real constraints, not product shape. Library and serve paths return failures instead of exiting or installing process-global state.
  - CI passes `--locked` on builds and tests and fails when a build dirties the tree.

</project-survey>
<execution-plan>

## Execution Instructions

- Status: decomposed on 2026-09-28 against local `master` at `a05d5cbd` ("Honor declared input and output files in the harness"), the commit every fact above was checked against. The worktree was clean, `vibe/ACTIVE` was absent, and `git log a05d5cbd..HEAD -- crates/promptforge-internal crates/promptforge crates/build-xtask` listed nothing, so every cited line holds when the run starts. If commits land before then, rerun that log and recheck each premise in the files they touch.
- Path: Full. Twelve components and 21 steps: one step per work item, in the run order the plan fixed, run one at a time. Each step's Todo line names the frontmatter todo it builds. Steps 1 to 12 are fixes. Steps 13 to 21 are pure-move splits, and each crate's marker lands in its last split step.
- Found during decomposition and built into the steps:
  - `Cancelled` is on the facade and the listing renders `Drop` impls, so Step 1 deregisters through a private field and leaves the listing unchanged (see the Decision Record).
  - `crates/promptforge/src/model.md` also says, at lines 569 and 601, that the two constructors cannot fail. Step 2 corrects both.
  - Narrowing the engine's `model` and `parser` modules sets off `unreachable_pub`, `unused_imports`, and private intra-doc link failures under the `-D warnings` gates. Step 10 resolves them.
- Component order, with the reason for each placement. Components 9 to 12 do not depend on one another, because a split changes no public path, so they keep the plan's order:
  1. **Types** (`types`), Step 1 - first. `promptforge-types` depends on no sibling, and no other step needs anything from it, so it keeps the plan's place.
  2. **Model client** (`model-client`), Step 2 - its constructor callers and `ClientError` wildcard arms sit in engine, Lua, `harness-models`, and facade files, so it lands before any of those crates is split.
  3. **Lua sandbox** (`lua-cancel`, `lua-cleanup`), Steps 3 and 4 - `promptforge-lua` sits on types and model-client but needs nothing from Steps 1 and 2, so it keeps the plan's place.
  4. **VFS safety** (`vfs-host`, `vfs-claims`), Steps 5 and 6 - `promptforge-vfs` depends on nothing, so no earlier step feeds it. It keeps the plan's place, ahead of Step 7, which also edits `handle.rs`.
  5. **Dead API removal** (`dead-api`), Step 7 - it edits the VFS crate, the engine, and the facade, and depends on no other step. It keeps the plan's place after the VFS fixes and is the second of the two listing changes.
  6. **Parser validation** (`parser-frontmatter`, `parser-structure`), Steps 8 and 9 - `promptforge-parser` sits on types and lua and needs nothing from Steps 1 to 7. It lands before the engine fixes, so the engine suite in Steps 10 and 11 parses its fixtures through the fixed parser.
  7. **Engine fixes** (`engine-surface`, `engine-scheduler`), Steps 10 and 11 - the engine depends on all five siblings, so its fixes come last among the crate fixes. Step 10 also adds a test to the Lua crate's `prelude-tests.rs`.
  8. **Housekeeping** (`housekeeping`), Step 12 - last fix. Its comment sweep reaches five of the six crates, including files earlier steps edit, such as the engine's `src/error.rs` (Step 2), so it follows every fix and precedes every split, and each file is split once.
  9. **Small-crate ceiling** (`split-small`), Step 13 - first split, after every fix. Types, model client, and parser hold 4,405 oversized lines together, few enough for one step with their three markers.
  10. **Lua ceiling** (`split-lua-tests`, `split-lua-rest`), Steps 14 and 15.
  11. **Engine ceiling** (`split-engine-source`, `split-engine-exec-flow`, `split-engine-scheduler-tests`, `split-engine-other-tests`), Steps 16 to 19.
  12. **VFS ceiling** (`split-vfs-handle`, `split-vfs-rest`), Steps 20 and 21 - last, because `handle.rs` is the riskiest split and Step 21 runs the full exit gate.
- Standing rules for every step. They are for the session that runs the plan. Each sub-agent reads only its own contract ranges and its step, so every rule a step needs also appears in Technical Design, the Project Survey, or the step itself, and each step's Read line names only those.
  - Each step is one commit holding its code, docs, and tests, and leaves every gate it runs passing.
  - Line numbers are at `a05d5cbd`. Steps shift lines in files later steps edit, such as the engine's `src/error.rs` (Steps 2 and 12), `vfs/src/handle.rs` (Steps 6 and 7), and the facade's `lib.md` (Steps 7 and 9), so re-locate each reference by the function, item, or heading it names.
  - Verification: on its own, the verifier runs the focused tests and the touched packages' nextest and doctest suites, adds the formatter and clippy only at a component's last step, and runs docs and the API check only in Step 21's full run. It also runs every command a step's Tests line names, as written. So each step ends its Tests line with a Gate commands list holding every other check it needs:
    - the formatter check and clippy for each touched package, so no step's lint or format fix lands in a later commit;
    - the suites of untouched packages the step can break;
    - docs builds where docs or intra-doc link targets change, with `$env:RUSTDOCFLAGS='-D warnings'` set first in PowerShell;
    - `cargo +nightly-2026-09-05 xtask api --check` where the public surface could move;
    - `cargo test -p build-xtask` where a crate's marker lands or `build-xtask` changes.
  - Steps 2 and 7 change facade items other crates may name, so they run the workspace clippy command that excludes the workshop trio, which compiles every other crate. The workshop clippy waits for Step 21: no workshop crate names `ClientError`, grep, or `max_depth`, and its build needs the UI and gateway-sidecar setup.
  - Step 21 runs every Exit criteria command.
  - The listing changes only in Steps 2 and 7. Each runs `cargo +nightly-2026-09-05 xtask api --bless` and reviews the diff in its own commit. Every other step passes `xtask api --check` with no listing change.
  - Split rules for Steps 13 to 21, from Technical Design, Structure, Split method, and Enforcement:
    - Count the lines of every `.rs` file in the crate first, as `text.lines().count()`, and work from those sizes, not the `a05d5cbd` figures. A file the fixes pushed past 500 that no split step names goes to its crate's last split step.
    - Read the crate's `test-count` from `cargo nextest list --locked -p <crate> --all-features --message-format json` before the first edit and after the last. The count after is no lower.
    - Moves only: moved code stays byte-identical apart from `use` lines, module wiring, and visibility, which takes the narrowest that compiles (`pub(super)` or `pub(crate)`). No public path changes, and no test moves between crates.
    - Layout follows the flat-directory rule in `AGENTS.md`: one or two files beside a module are `foo-bar.rs` siblings wired with `#[path]`, and three or more become a `foo/` directory in standard layout, as in `engine/src/execute/run/`. A file over only because of its inline test module moves that module to a kebab-named `-tests.rs` sibling wired with `#[path]`, and a moved test module still over 500 splits again by topic. Source files split along their existing seams, and test files by the topic groups they already hold.
    - A crate's marker lands in the split step that brings its last file under the ceiling: a `//! ## Invariants` section in `src/lib.rs`, shaped like the `new-crate` template (`crates/build-xtask/src/new_crate.rs`, lines 66 to 76) without its Tier wording. Its two bullets name what the crate may depend on, as `crates/promptforge-internal/README.md` lists it, and say "Every file in this crate stays under 500 lines; split first, then edit."
  - Not done here: everything under Deferred and Out of Scope.
  - Writing: plain English, single dashes only, never em dashes or double dashes.

<step-1>

### Step 1: Deregister dropped cancellation futures, fold `from_validated`, and fill the event test gaps [completed]

- Component: Types
- Piece: the three `promptforge-types` items, built jointly in one commit. They share no code path, but all three sit in one crate, and one run of its unit tests covers them.
- Todo: `types`
- Depends on: nothing
- Read: Technical Design, the `promptforge-types` bullets; Project Survey.
- Build, under `crates/promptforge-internal/types/src/`:
  - `cancel.rs`: each `Cancelled` takes a unique key from an atomic counter when it is created. Each node's `Mutex<Vec<Waker>>` holds `(key, Waker)` pairs instead, deduplicated per key, and a re-poll whose waker does not `will_wake` the stored one replaces that key's entry. Dropping a `Cancelled` removes its key's entries under each node's mutex, a no-op once `cancel` has drained the list. The removal sits in the `Drop` of a private field type that `Cancelled` holds, not in an `impl Drop for Cancelled`, and that field keeps every auto trait the listing records for `Cancelled` (`Freeze`, `RefUnwindSafe`, `Send`, `Sync`, `Unpin`, `UnwindSafe`), so the listing does not change.
  - `models.rs`: `ModelCatalog::from_validated` (line 292) folds into `empty()` (line 299), its only caller.
- Tests:
  - `cancel-tests.rs`: a `Cancelled` dropped before cancel leaves no waker on any ancestor; of two `Cancelled` futures on one handle polled from the same task, dropping one leaves the other woken by a later cancel; a future re-polled with a different waker is woken through the new waker only.
  - `event-tests.rs`: every `Event` variant round-trips (nine do today, lines 47 to 143), and an exhaustive `match` maps every payload-free variant to its lifecycle constant from `event-lifecycle.rs`, so a new variant fails to compile until it is mapped.
  - Focused command: `cargo nextest run --locked -p promptforge-types --all-features`.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-types --all-targets --all-features -- -D warnings`; `cargo nextest run --locked -p promptforge-engine --all-features`; `cargo +nightly-2026-09-05 xtask api --check`.
- Verify: `promptforge-types`, plus the engine suite, whose cancellation tests await `Cancelled` futures. `xtask api --check` passing with no listing change confirms the `Drop` placement.
- Commit: one commit with the three items and their tests.
- Done when: the new tests pass, the types and engine suites pass, and the listing is unchanged.

</step-1>

<step-2>

### Step 2: Validate host-built completions, fix the SSE scan, and mark `ClientError` non-exhaustive [completed]

- Component: Model client
- Piece: the `promptforge-model-client` fixes, built jointly in one commit. The validating constructors must land with every caller they break, and `#[non_exhaustive]` with every wildcard arm it forces. One test set covers the whole item: the crate's unit tests with the engine tests and facade doctests that build completions.
- Todo: `model-client`
- Depends on: nothing
- Read: Technical Design, the `promptforge-model-client` bullets and File and public API changes; Project Survey.
- Build, under `crates/promptforge-internal/model-client/src/` unless a path says otherwise:
  - `normalize.rs`: each check the constructors need that is written inline (blank call id, blank name, non-object arguments, empty tool-call batch, duplicate call ids) is extracted into a small function that `normalize.rs` keeps calling. `normalize.rs` calls neither constructor.
  - `client/wire-canned.rs` (lines 16 to 47): `Completion::from_result` and `ToolCall::from_parts` keep their argument types, return `Result<_, ClientError>`, and apply those functions, raising the variant the live normalizer raises for each rule. A text result is not validated.
  - Every caller, in the same commit: `crates/promptforge-internal/engine/src/execute/tests/serial_driver.rs` (lines 28 and 37 to 38), `crates/promptforge-internal/engine/src/execute/run/effect-tests.rs` (line 92), and the compiled examples in `crates/promptforge/src/model.md` (line 81 and lines 328 to 333) and `crates/promptforge/src/effect.md` (lines 64 and 174).
  - `crates/promptforge/src/model.md` prose: the paragraph that introduces the constructors (line 322) gains one sentence: both refuse, with a `ClientError`, a blank call id or name, arguments that are not a JSON object, an empty tool-call batch, and duplicate call ids. The reference entries for `from_result` (line 569) and `from_parts` (line 601) each say it "cannot fail"; both are restated to name what it refuses.
  - `client/stream.rs`: `SseScanner::next_data` (lines 60 to 74) keeps the offset it already scanned without finding `\n`, resumes there, and resets it on drain. The error envelope check (lines 162 to 170) counts only a present, non-null `error`.
  - `error.rs` (lines 43 to 151): `ClientError` gains `#[non_exhaustive]`, with its variants and field visibility unchanged. Each exhaustive `match` the compiler then reports in another crate gains a wildcard arm: `RunErrorKind::Completion` in the `RunErrorKind` mapping (`crates/promptforge-internal/engine/src/execute/error.rs`, around lines 104 to 110), `ErrorKind::Internal` in the Lua error-kind mapping (`crates/promptforge-internal/engine/src/error.rs`, around lines 805 to 820 and 852), and the outcome of its most general existing arm anywhere else, such as the `From` impl in `crates/promptforge-internal/lua/src/error.rs` (line 223) or `harness-models`, which imports the type as `Error` (`crates/harness-internal/models/src/transport.rs`, `config.rs`, and `catalog.rs`). The commit message names each arm and its outcome.
  - Run `cargo +nightly-2026-09-05 xtask api --bless`. The listing diff shows only the two new signatures and `#[non_exhaustive]` on `ClientError`.
- Tests:
  - `from_parts` refuses a blank id, a blank name, and non-object arguments; `from_result` refuses an empty tool-call batch and duplicate call ids, and accepts a text result.
  - `client/stream-tests.rs`: a `data:` line delivered in one-byte reads is returned whole, and only once, when its newline arrives, with no timing or byte-count assertion; a chunk with `"error": null` parses as an ordinary chunk; `error_envelope_fails_the_stream_with_the_escaped_message` still passes.
  - Focused commands: `cargo nextest run --locked -p promptforge-model-client --all-features` and `cargo test --locked --doc -p promptforge --all-features`.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`; under the survey's `RUSTDOCFLAGS` setting, cleared afterwards, `cargo doc --locked --no-deps --all-features -p promptforge-model-client -p promptforge-engine -p promptforge`, `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`, and `cargo doc --locked --no-deps -p promptforge`; `cargo +nightly-2026-09-05 xtask api --check`.
- Verify: the workspace clippy command without the workshop trio, which compiles every crate that could hold an exhaustive `ClientError` match; the component test pattern for `promptforge-model-client`, `promptforge-lua`, `promptforge-engine`, `harness-models`, and `promptforge`; the docs commands for `promptforge-model-client`, `promptforge-engine`, and `promptforge`; `xtask api --check` against the re-blessed listing.
- Commit: one commit with the model-client changes, every caller and wildcard arm, the facade docs, the re-blessed listing, and the tests.
- Done when: the new tests pass, every listed suite passes, and the listing diff holds exactly the model-client changes File and public API changes names.

</step-2>

<step-3>

### Step 3: Keep cancellation ahead of `xpcall` handlers and refuse finalizers [completed]

- Component: Lua sandbox
- Piece: Lua cancellation, the first of the component's two pieces, built sequentially. The pieces share no file and have separate test sets, and cancellation goes first, as the plan orders.
- Todo: `lua-cancel`
- Depends on: nothing
- Read: Technical Design, the `__impl_coro.lua` and `__impl_globals.lua` bullets; Project Survey.
- Build, under `crates/promptforge-internal/lua/src/`:
  - `__impl_coro.lua`, `protected_xcall` (lines 92 to 99): when `cancel_requested()` is set, the function-handler wrapper returns the failure unchanged without calling the author handler, and otherwise returns `handler(normalize_failure(failure))`. `xpcall_outcome` then re-raises. No other shim path changes.
  - `__impl_globals.lua`, `replace_metatable` (lines 90 to 108): for any target, a table metatable with `rawget(metatable, "__gc") ~= nil` is refused at level 2 with `setmetatable: finalizers (__gc) are not available in the sandbox`, and for `_G`, one with `rawget(metatable, "__mode") ~= nil` is refused with `setmetatable: weak tables (__mode) are not available for _G`. `forward` (lines 72 to 80) stops copying `__mode` onto the guard.
- Tests, in `tests.rs` beside the cancellation tests near line 2285, each on a VM built with `shim_vm` (line 2246), which installs the coroutine shims and takes an optional cancel handle:
  - `xpcall(function() while true do end end, function() while true do end end)` ends as interrupted under a set cancel flag;
  - a non-looping handler still receives an ordinary failure;
  - `setmetatable({}, { __gc = f })` and `setmetatable(_G, { __mode = "v" })` fail with their messages, and `setmetatable(_G, { __index = f })` still works;
  - `long_running_lua_block_cancels_cooperatively` and `a_pre_cancelled_run_aborts_a_tight_loop_promptly` pass unchanged.
  - Focused command: `cargo nextest run --locked -p promptforge-lua --all-features`.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-lua --all-targets --all-features -- -D warnings`; `cargo nextest run --locked -p promptforge-engine --all-features`.
- Verify: `promptforge-lua`, plus the component test pattern for `promptforge-engine`, whose suite runs every section through these shims.
- Commit: one commit with the two shim changes and the tests.
- Done when: the looping-handler test ends as interrupted instead of running into the nextest timeout, and the Lua and engine suites pass.

</step-3>

<step-4>

### Step 4: Pin `var` key removal, delete the unreachable `models.infer` placeholder, and correct the Lua docs [completed]

- Component: Lua sandbox
- Piece: Lua cleanup, the second piece, built after Step 3, with which it shares no file.
- Todo: `lua-cleanup`
- Depends on: nothing
- Read: Technical Design, the `promptforge-lua` bullets on the `models.infer` placeholder, the store doc, `var.k = nil`, and the crate docs; Project Survey.
- Build:
  - Delete the Rust placeholder behind `models.infer`, under `crates/promptforge-internal/lua/src/`: the placeholder closure and `call_models_infer_hook` in `install_models` (`models.rs`, lines 57 to 72 and 351 to 354), `ModelsInferHook` (`models-userdata.rs`, lines 15 to 23), `clear_infer_hook` and its teardown call (`vm.rs`, lines 569 to 575 and 1052 to 1075), and `ModelsInferHook` in the shared re-export lines of `models.rs` (line 26) and `lib.rs` (line 57), which keep `LuaModelHandle`, and in the `vm.rs` import list (line 16). The shim `infer` in `__impl_coro.lua` (lines 101 to 117, assigned at lines 393 to 394) stays, so `models.infer` is unchanged for authors.
  - Remove the stale "infer hook" comments in `crates/promptforge-internal/engine/src/execute/tests/suite/exec_flow.rs`.
  - `lua/src/host.rs` (lines 641 to 643): the `run_store_op` doc stops calling it the single implementation. It says the direct closures (lines 470 to 678) run only during shared-library load, before `route_store_to_shims`, share its VFS operation bodies, and add lifecycle events, store-conflict recording, and `Error::store` wrapping. No code changes.
  - `lua/src/lib.rs` crate docs: add capability preludes, `input`, and `tasks`.
  - The `__newindex` in `lua/src/sys.rs` changes only if the `var.k = nil` test fails.
- Tests:
  - `var.k = nil` removes `k` from `var` and from its snapshot. This pins the traced path, which already removes the key.
  - `the_models_namespace_has_no_bind` (`lua/src/models-tests.rs`, lines 70 to 84) no longer expects `install_models` alone to define `models.infer`.
  - Every engine test that calls `models.infer` passes unchanged: about 130 call sites in 23 files, including `execute/tests/live_infer.rs` and `execute/tests/scheduler/live_h1.rs`. This is the guard that `models.infer` itself survives.
  - Focused commands: `cargo nextest run --locked -p promptforge-lua --all-features` and `cargo nextest run --locked -p promptforge-engine --all-features`.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-lua -p promptforge-engine --all-targets --all-features -- -D warnings`.
- Verify: `promptforge-lua` and `promptforge-engine`.
- Commit: one commit with the deletions, the doc corrections, and the tests.
- Done when: the Lua and engine suites pass, and `ModelsInferHook`, `call_models_infer_hook`, and `clear_infer_hook` appear nowhere under `crates/`.

</step-4>

<step-5>

### Step 5: Refuse dangling symlinks in the host backend [completed]

- Component: VFS safety
- Piece: the dangling-link refusal, the first of two pieces built sequentially. The pieces share no file (`host.rs` here, `handle.rs` in Step 6) and have separate test sets.
- Todo: `vfs-host`
- Depends on: nothing
- Read: Technical Design, the `host.rs` `contain` bullets; Project Survey.
- Build, under `crates/promptforge-internal/vfs/`:
  - `src/host.rs`, `contain` (lines 130 to 159): inside the walk-up loop, before stepping to the parent, refuse with `VfsError::PermissionDenied` and the reason `<path> passes through a dangling symbolic link` when `ancestor.symlink_metadata().is_ok()` and `!ancestor.exists()`. That covers `read`, `read_range`, `write`, `append`, `list`, `glob`, and `copy`, and, through `contain_no_follow` (lines 166 to 183), a dangling link in a no-follow operation's parent path. A final-component link is still addressed as a link.
  - `README.md` (line 9) gains: content operations refuse a path that passes through a dangling symbolic link.
- Tests, in `host.rs`'s inline `mod tests` (from line 707). Make each dangling link with `make_dir_link` to a directory the test then removes. On Windows that is a junction, which needs no symlink privilege, so the refusal runs on every host. A case that needs a file link skips with a logged reason when Windows refuses with raw OS error 1314.
  - a dangling link inside the root refuses `append`, `write`, `read`, and `list`, and no file appears at the link's target;
  - `remove` and `exists` still act on the dangling link itself;
  - the Windows escape test (lines 875 to 907) logs a skip instead of passing silently, and also covers `append`;
  - the test near line 1034 that is named for a `..` clamp is renamed for what it asserts.
  - Focused command: `cargo nextest run --locked -p promptforge-vfs --all-features host::tests`.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-vfs --all-targets --all-features -- -D warnings`.
- Verify: `promptforge-vfs`.
- Commit: one commit with the `contain` change, the README sentence, and the tests.
- Done when: the junction-based dangling-link tests run and pass on Windows as well as Unix, and the VFS suite passes.

</step-5>

<step-6>

### Step 6: Close the three claims-ledger gaps [completed]

- Component: VFS safety
- Piece: the claims gaps, the second piece, built after Step 5.
- Todo: `vfs-claims`
- Depends on: nothing
- Read: Technical Design, the `handle.rs` claims-gaps bullets; Project Survey.
- Build, in `crates/promptforge-internal/vfs/src/handle.rs`. Each new conflict uses the happens-before comparison the neighboring checks apply to a recorded claim, so ordered accesses from one task or across a join never conflict.
  - `check_subtree` (lines 838 to 916) also scans `tables.subtrees` and conflicts when `subtree_covers(path, other)` or `subtree_covers(other, path)` (`subtree_covers`, lines 1002 to 1008), and also conflicts with a listing of `parent_of(path)` (lines 968 to 978). Its callers, `claim_subtree`, `claim_rename`, and the recursive branch of `Access::remove`, need no change.
  - `claim_list` (lines 680 to 709) also conflicts with a subtree claim whose root is a direct child of the listed directory, `parent_of(subtree) == dir`, and never through `subtree_covers(dir, subtree)`, which would also match grandchildren. The check at lines 690 to 691 already covers subtrees that contain the directory.
  - `check_write` (lines 486 to 585), in the `may_create` loop (lines 558 to 583): each created ancestor is also checked against `tables.children` for its parent and against `tables.patterns` through `pattern_matches_path` (lines 1043 to 1051), as the leaf already is (lines 523 to 541).
- Tests, in `handle.rs`'s claims tests (the inline `mod tests` from line 2578), each with two tasks not ordered by a join, in both orders:
  - a recursive remove of `/a` against a recursive remove, and against a rename, of `/a/b` conflicts;
  - `list("/a")` against `remove("/a/b", true)` conflicts;
  - `list("/a")` against `remove("/a/b/c", true)` does not conflict, because the listing of `/a` is unchanged;
  - a write that creates `/a/b/c` against `list("/a")`, and against `glob("/a/*")`, conflicts;
  - each conflicting pair with a join between the tasks does not conflict.
  - Focused command: `cargo nextest run --locked -p promptforge-vfs --all-features handle::tests`.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-vfs --all-targets --all-features -- -D warnings`; `cargo nextest run --locked -p promptforge-engine --all-features`.
- Verify: `promptforge-vfs`, plus the component test pattern for `promptforge-engine`, since the new checks can turn a passing concurrent prompt into a claims conflict.
- Commit: one commit with the three checks and the tests.
- Done when: each conflicting pair conflicts in both orders, the non-conflicting and joined cases pass, and the VFS and engine suites pass.

</step-6>

<step-7>

### Step 7: Remove grep and `Environment::max_depth` from the public surface [completed]

- Component: Dead API removal
- Piece: the grep removal and the `max_depth` removal, built jointly in one commit. Each must land with its facade re-exports, docs, and listing change, the two share one re-bless, and one pass of the gates is their test set.
- Todo: `dead-api`
- Depends on: nothing
- Read: Technical Design, the grep-removal bullets, the `Environment::max_depth` bullet, and File and public API changes; Project Survey.
- Build:
  - Under `crates/promptforge-internal/vfs/src/`, delete: `grep.rs` with its `mod` and `pub use` in `lib.rs` (lines 19 and 32); `VfsAccess::grep` (`traits.rs`, lines 309 to 360) and `Op::Grep` (line 426); `Access::grep` (`handle.rs`, lines 2110 to 2138); the forwarding `grep` methods on `HandleAccess`, `StoreMountSession`, and `StoreScoped` (`handle.rs`, lines 2354 to 2355, 2452 to 2453, and 2556 to 2557); `RoutingAccess::grep` (`router.rs`, lines 318 to 331); grep's tests in `traits.rs` (lines 506 to 507 and 723 to 780) and `detail.rs` (lines 480 to 512); and the grep mention in the `path.rs` doc (line 57).
  - `crates/promptforge/src/lib.rs`: remove the `GrepMatch`, `GrepQuery`, and `GrepResults` re-exports (lines 149 to 151). `Access` and `VfsAccess` stay.
  - `crates/promptforge/src/vfs.md`: remove the `GrepQuery`, `GrepResults`, and `GrepMatch` sections (from line 665), the `Access::grep` and `VfsAccess::grep` entries (lines 558 and 881), and the grep mentions at lines 379, 667, and 729.
  - `crates/promptforge-internal/engine/src/execute/environment.rs`: remove the `max_depth` field, default, setter, and `Debug` entry (lines 34, 51, 61 to 62, and 144), and drop every test call of the setter. The call depth cap stays the `MAX_CALL_DEPTH` constant in `scheduler/tasks.rs`.
  - `crates/promptforge/src/lib.md`: remove the `max_depth` text (line 398).
  - Run `cargo +nightly-2026-09-05 xtask api --bless`. The diff removes only the grep items (lines 74 to 76, 185 to 187, 310 to 312, 645, 683, 1290 to 1300, and 1310) and `Environment::max_depth` (line 420).
- Tests: none added. The grep tests leave with the code, and the remaining VFS, engine, and facade suites, the docs builds that deny broken intra-doc links, and the listing check cover the removals.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`; under the survey's `RUSTDOCFLAGS` setting, cleared afterwards, `cargo doc --locked --no-deps --all-features -p promptforge-vfs -p promptforge-engine -p promptforge`, `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`, and `cargo doc --locked --no-deps -p promptforge`; `cargo +nightly-2026-09-05 xtask api --check`.
- Verify: the workspace clippy command without the workshop trio, which compiles every crate that could name a removed item; the component test pattern for `promptforge-vfs`, `promptforge-engine`, and `promptforge`; the docs commands for the same three; `xtask api --check` against the re-blessed listing.
- Commit: one commit with both removals, their docs, and the re-blessed listing.
- Done when: `GrepQuery`, `GrepResults`, `GrepMatch`, and `Op::Grep` appear nowhere under `crates/`, `max_depth` is gone from `environment.rs`, the facade's `lib.md`, and the listing, and every listed gate passes.

</step-7>

<step-8>

### Step 8: Refuse duplicate capabilities and tool slots backed by optional capabilities [completed]

- Component: Parser validation
- Piece: the two frontmatter checks, the first of two pieces built sequentially. Both pieces edit `parse.rs`, and their tests sit in separate files (`contract/tests.rs` here, `tests.rs` in Step 9).
- Todo: `parser-frontmatter`
- Depends on: nothing
- Read: Technical Design, the Optional-capability slots and Duplicate capabilities bullets and the Guide bullets under File and public API changes; Project Survey.
- Build:
  - `crates/promptforge-internal/parser/src/contract.rs`: two new checks beside `check_distinct_aliases` (lines 379 to 392), each raising `Error::parse(ParseErrorKind::Frontmatter, message)`:
    - the duplicate check refuses a `capabilities:` list whose decoded ids repeat, whatever each entry's form, `optional` flag, and `config`, with `invalid frontmatter: capability <capability> is declared more than once under capabilities`;
    - the slot check looks up, for each `ToolSlot::Exact(id)`, a `CapabilityDecl` whose id equals `id.capability()` and whose `is_optional()` is true, and refuses with `invalid frontmatter: tool alias '<alias>' names <tool id>, whose capability <capability> is declared optional; a tool slot requires its capability`.
  - `parser/src/parse.rs`: call the duplicate check, then the slot check, next to the existing alias check (lines 93 to 94), after the frontmatter is decoded.
  - `guide/src/language/12-tools.md`: "Declaring capabilities" says each capability is declared once, and it and "Tool slots and Tool objects" say a tool slot requires its capability, so an optional capability cannot back one. `guide/src/language/02-file-structure.md`, "Frontmatter rules and errors", gains both refusals.
  - Regenerate `guide/promptforge-language-guide.md` with `cargo run --locked -q -p build-user-guide`.
  - Nothing in the tree lists a capability twice or binds a slot to an optional capability, so no prompt, fixture, or guide example changes, and the engine's run-time refusal in `crates/promptforge-internal/engine/src/execute/fill.rs` (lines 42 to 54) stays.
- Tests, in `parser/src/contract/tests.rs`:
  - the slot refusal with its exact message, and a prompt that declares an optional capability without a slot still parses;
  - the duplicate refusal with its exact message for two plain entries, two optional ones, one of each, and two map entries that differ only in `config`; a duplicate that also backs a slot reports the duplicate; a list of distinct capabilities still parses.
  - Focused command: `cargo nextest run --locked -p promptforge-parser --all-features contract`.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-parser --all-targets --all-features -- -D warnings`; `cargo nextest run --locked -p promptforge-engine --all-features`; `cargo run --locked -q -p build-user-guide`, then `git diff --exit-code -- guide/promptforge-language-guide.md`.
- Verify: `promptforge-parser`, plus the component test pattern for `promptforge-engine`, whose suite parses every fixture prompt.
- Commit: one commit with the checks, the guide text, the regenerated combined guide, and the tests.
- Done when: the new tests pass, the parser and engine suites pass, and rerunning `build-user-guide` leaves no diff.

</step-8>

<step-9>

### Step 9: Close the frontmatter only at column 0, skip container headings, and locate structural errors [completed]

- Component: Parser validation
- Piece: the structure fixes, the second piece, built after Step 8, which also edits `parse.rs`.
- Todo: `parser-structure`
- Depends on: nothing
- Read: Technical Design, the Span documentation, `split_frontmatter`, `collect_headings`, and Structural error locations bullets, and the `ParseError::span` item under File and public API changes; Project Survey.
- Build:
  - `crates/promptforge-internal/parser/src/error.rs` and the `ParseError::span` doc in `crates/promptforge/src/lib.md`: the range is relative to the document body after the frontmatter and a leading BOM, with CRLF normalized to LF, while `line()` and `column()` locate the error in the original file. No code changes.
  - `parser/src/build.rs`, `split_frontmatter` (line 281): the closer test becomes `line.trim_end() == "---"`.
  - `build.rs`, `collect_headings` (lines 354 to 387): track nesting depth over `Tag::BlockQuote` and `Tag::Item` start and end events. A `Tag::Heading` inside either is not a section boundary, and its text stays in the enclosing section's prose.
  - `build.rs`, the orphan heading and empty heading errors (lines 483 to 500): built the way the duplicate-sibling error is (lines 532 to 541), with the heading's body-relative span, so `with_prompt_context` fills in line and column.
  - `parser/src/parse.rs`, the "`lua shared` outside H1" error (lines 134 to 146): built the same way, with the fence's range from `shared_fences`. The unclosed-fence error in `fence.rs` (line 148) stays as it is.
- Tests, in `parser/src/tests.rs`:
  - an indented `---` inside a YAML block scalar does not close the frontmatter;
  - a heading inside a block quote, and one inside a list item, is not a section, and its text stays in the enclosing section's prose;
  - the orphan heading, empty heading, and "`lua shared` outside H1" errors report a line.
  - Focused command: `cargo nextest run --locked -p promptforge-parser --all-features`.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-parser --all-targets --all-features -- -D warnings`; `cargo nextest run --locked -p promptforge-engine --all-features`; under the survey's `RUSTDOCFLAGS` setting, cleared afterwards, `cargo doc --locked --no-deps --all-features -p promptforge-parser -p promptforge` and `cargo doc --locked --no-deps -p promptforge`.
- Verify: `promptforge-parser`, plus the component test pattern for `promptforge-engine` and `promptforge` and the facade docs commands, since the facade's `lib.md` changed.
- Commit: one commit with the three code fixes, the two span docs, and the tests.
- Done when: the new tests pass and the parser, engine, and facade suites pass.

</step-9>

<step-10>

### Step 10: Narrow the engine's unused public surface and fill two test gaps [completed]

- Component: Engine fixes
- Piece: the surface narrowing with its test gaps, the first of two pieces built sequentially. Both pieces edit `src/lib.rs`, and their tests are separate.
- Todo: `engine-surface`
- Depends on: nothing
- Read: Technical Design, the engine Surface and `Cargo.toml` bullets; Project Survey.
- Build, under `crates/promptforge-internal/engine/`:
  - `src/lib.rs`: `pub mod model` and `pub mod parser` (lines 8 and 9) become `pub(crate)`. The root re-exports of `CompletionError`, `CompletionErrorKind`, `ParseError`, `ParseErrorKind`, `Prompt`, and `promptforge_version` (lines 29 and 30) stop being public: `pub(crate) use` where engine code imports them through the crate root, deleted otherwise. `StoreOp` and `StoreOutcome` leave the public `pub use crate::execute::{..}` list (lines 22 to 28).
  - `src/execute.rs` (line 71): `pub use promptforge_lua::{StoreOp, StoreOutcome}` becomes `pub(crate) use`, and its comment (lines 68 to 70) no longer says a host names them here. `perform_store_op` stays public, and the facade already takes both types from `promptforge_lua` (`crates/promptforge/src/lib.rs`, lines 140 to 142).
  - Lint fallout: once the modules are `pub(crate)`, `unreachable_pub` reports the `pub use` lists in `src/model.rs` (lines 23 to 35) and `src/parser.rs` (lines 23 to 29). Each becomes `pub(crate) use`, and any name `unused_imports` then reports is deleted.
  - Doc fallout: the crate docs in `src/lib.md` link `parser::Prompt`, `parser`, `model`, `promptforge_version`, and `Prompt` (lines 3, 5, and 9), which become private intra-doc links that the docs gate refuses. Each is pointed at its home crate, such as `promptforge_parser::Prompt`, or unlinked, and so is any other public doc link into the narrowed items that the docs gate reports.
  - `src/lib.md` doctests (lines 12 and 26): use `promptforge::` paths through the doctest-only facade dev-dependency.
  - `benches/models_loop.rs` (lines 31 to 35): import from `promptforge_model_client` and `promptforge_parser`, already normal dependencies.
  - `Cargo.toml` (lines 31 to 37): the `test-support` comment says the feature exists only for `benches/models_loop.rs` (`required-features`, line 65).
- Tests:
  - `src/execute/requirements-tests.rs`: the conflict line of `Requirements::notice` has its wording and position pinned.
  - `crates/promptforge-internal/lua/src/prelude-tests.rs`: a capability prelude global that collides with `tools`, `store`, or `models` is refused.
  - Focused commands: `cargo nextest run --locked -p promptforge-engine --all-features requirements` and `cargo nextest run --locked -p promptforge-lua --all-features prelude`.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-engine -p promptforge-lua -p promptforge --all-targets --all-features -- -D warnings`; under the survey's `RUSTDOCFLAGS` setting, cleared afterwards, `cargo doc --locked --no-deps --all-features -p promptforge-engine` and `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`; `cargo +nightly-2026-09-05 xtask api --check`.
- Verify: `promptforge-engine`, whose `--all-targets` clippy run builds the bench, and `promptforge-lua`; the facade's clippy build, which compiles it against the narrowed engine; the engine's docs builds; `xtask api --check` with no listing change.
- Commit: one commit with the narrowing, its lint and doc fallout, the bench and doctest paths, the `Cargo.toml` comment, and the two tests.
- Done when: nothing outside the engine names the narrowed items, the listing is unchanged, and every listed gate passes.

</step-10>

<step-11>

### Step 11: Fix the `entered` placeholder, share the `Chain` constructor, and rename `fanout.rs` [completed]

- Component: Engine fixes
- Piece: the scheduler fixes, the second piece, built after Step 10, which also edits `src/lib.rs`.
- Todo: `engine-scheduler`
- Depends on: nothing
- Read: Technical Design, the engine `entered`, `start_live_h1`, `drive.rs`, and `fanout.rs` bullets and the Renamed item under File and public API changes; Project Survey.
- Build, under `crates/promptforge-internal/engine/src/`:
  - `execute/scheduler/chain.rs` (line 77) and `execute/scheduler/h1.rs` (line 64): initialize `entered` from the section at the chain's start index, falling back to the prompt title when the index is past the slice.
  - `start_live_h1` (`h1.rs`, lines 48 to 81) and `start_chain` (`chain.rs`, lines 78 to 111) build their `Chain` through one shared constructor.
  - `execute/scheduler/drive.rs` (line 205): the loop iterates the arena's indices directly, so the `unwrap_or_else(|_| panic!(..))` conversion goes.
  - Rename `fanout.rs`, which holds only `parse_heading_address` and `resolve_sibling`, to `heading_address.rs`, updating `lib.rs` (line 6), `execute/engine.rs` (lines 13, 59, and 96), `h1.rs` (lines 18 and 173), and `execute/scheduler/walk.rs` (lines 18 and 291). The test file `execute/tests/scheduler/fanout.rs` keeps its name.
- Tests: a chain that starts past index 0 names its start section as `entered`. The unchanged scheduler suite covers the shared constructor, the panic removal, and the rename.
  - Focused command: `cargo nextest run --locked -p promptforge-engine --all-features scheduler`.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-engine --all-targets --all-features -- -D warnings`; under the survey's `RUSTDOCFLAGS` setting, cleared afterwards, `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`.
- Verify: `promptforge-engine`, including its private-items docs build, since the rename moves intra-doc link targets.
- Commit: one commit with the four scheduler changes and the test.
- Done when: the new test passes, the engine suite passes, and no `crate::fanout` path remains.

</step-11>

<step-12>

### Step 12: Fold two `build-xtask` directories, widen the VFS manifest test, and sweep audit comments [completed]

- Component: Housekeeping
- Piece: the two folds, the manifest-test forms, and the comment sweep, built jointly in one commit. None changes behavior outside a test, no piece depends on another, and one pass of the gates verifies all three.
- Todo: `housekeeping`
- Depends on: Steps 1 to 11, so the sweep sees every comment they left.
- Read: Technical Design, the `build-xtask` bullets, the `promptforge-vfs` manifest-test bullet, and the comment-sweep bullet; Project Survey, the flat-directory and comment conventions.
- Build:
  - `crates/build-xtask/src/product.rs` (lines 421 to 432): move `product-container-tests.rs`, `product-harness-tests.rs`, `product-test-support.rs`, and `product-tests.rs` to `product/container_tests.rs`, `product/harness_tests.rs`, `product/test_support.rs`, and `product/tests.rs`, and drop their `#[path]` attributes. `product.rs` stays the parent, as `engine/src/execute/run.rs` does beside `run/`.
  - `crates/build-xtask/src/api/`: move `listing-compact.rs` to `listing/compact.rs`, `listing-tests.rs` to `listing/tests.rs`, and `listing-compact-tests.rs` to `listing/compact-tests.rs`. `listing.rs` (lines 101 to 106) drops its `#[path]` attributes, and `compact.rs` wires its tests with `#[path = "compact-tests.rs"]`, as `run/effect.rs` wires `effect-tests.rs`.
  - `crates/promptforge-internal/vfs/src/lib.rs` (lines 172 to 211): `is_dependency_table` and `the_manifest_declares_no_dependencies` also catch `[target.<cfg>.dev-dependencies]`, `[target.<cfg>.build-dependencies]`, and `[target.<cfg>.dependencies.<name>]` tables.
  - The comment sweep across the six crates: restate audit tags such as `F3`, and history wording such as "legacy", "used to", "retired", and "formerly", as present-tense constraints, or delete them, per the comment rule in `AGENTS.md`.
    - Known sites: the engine's `src/error.rs` (lines 97, 948, and 974), `src/test_support/recording.rs` (line 36), `src/test_support/tokio_driver.rs` (line 36), and `src/execute/tests/scheduler/walk.rs`, and `model-client/src/detail.rs` (`(F7)` in the `tool_schema_new` doc).
    - Find the rest with `rg -n -g '*.rs' '(//|//!|///).*(\bF\d+\b|legacy|used to|retired|formerly)' crates/promptforge-internal`. At `a05d5cbd` it matched about 140 lines in 40 files, in every crate except `promptforge-vfs`, and 43 of those lines, in 19 files, hold audit tags. A match that is already present-tense prose, such as "the key used to look up", stays.
- Tests: fixture manifests using each newly caught form are refused by the manifest test. The folds and the sweep add no test, and the moved `build-xtask` tests pass unchanged.
  - Focused commands: `cargo test -p build-xtask` and `cargo nextest run --locked -p promptforge-vfs --all-features manifest`.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p build-xtask -p promptforge-types -p promptforge-model-client -p promptforge-lua -p promptforge-parser -p promptforge-engine -p promptforge-vfs --all-targets --all-features -- -D warnings`; under the survey's `RUSTDOCFLAGS` setting, cleared afterwards, `cargo doc --locked --no-deps --all-features -p promptforge-types -p promptforge-model-client -p promptforge-lua -p promptforge-parser -p promptforge-engine -p promptforge-vfs` and `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`, since the sweep edits doc comments; `cargo +nightly-2026-09-05 xtask api --check`, since the listing code itself moves.
- Verify: `build-xtask`, `promptforge-vfs`, and every crate the sweep edits.
- Commit: one commit with the folds, the manifest test, and the sweep.
- Done when: `crates/build-xtask/src/` holds no `product-*.rs` and `api/` no `listing-*.rs`, the sweep's search finds no audit tag or history wording left, and every listed gate passes.

</step-12>

<step-13>

### Step 13: Split `promptforge-types`, `promptforge-model-client`, and `promptforge-parser` to the ceiling [completed]

- Component: Small-crate ceiling
- Piece: the three crates' splits and markers, built jointly in one commit. They share no file, and together (4,405 oversized lines at `a05d5cbd`) one sub-agent can read every file they cut.
- Todo: `split-small`
- Depends on: Steps 1, 2, 8, 9, and 12, every fix that edits these crates.
- Read: Technical Design, Structure, Split method, and Enforcement; Project Survey.
- Build, under `crates/promptforge-internal/`:
  - Count, and read each crate's test count, first.
  - `types/src/untrusted-inventory.rs` (649 lines, 503 before its inline tests): the tests move to `untrusted-inventory-tests.rs`, and the source still needs one cut along an existing seam, since it is over 500 on its own.
  - `model-client/src/normalize.rs` (1,181, 466 before its inline tests, plus what Step 2 extracted): the tests move to a `-tests.rs` sibling, which splits again by topic.
  - `parser/src/build.rs` (572, 556 before its tests, plus Step 9's additions): the source splits along its seams, such as `split_frontmatter` and `collect_headings`, and the tests move out. `parser/src/tests.rs` (1,441, plus Step 9's tests) and `parser/src/contract/tests.rs` (562, plus Step 8's tests) split by their topic groups.
  - Any other file in the three crates that the count finds over 500, such as `types/src/event-tests.rs` after Step 1 or `parser/src/contract.rs` after Step 8.
  - Markers: `types/src/lib.rs` (no workspace dependency beyond `workspace-hack` and the doctest-only `promptforge` dev-dependency), `model-client/src/lib.rs` (`promptforge-types`), and `parser/src/lib.rs` (`promptforge-types` and `promptforge-lua`).
- Tests: none added. Each crate's test count after is no lower than before, and the coder's return states every count before and after.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-types -p promptforge-model-client -p promptforge-parser --all-targets --all-features -- -D warnings`; under the survey's `RUSTDOCFLAGS` setting, cleared afterwards, `cargo doc --locked --no-deps --all-features -p promptforge-types -p promptforge-model-client -p promptforge-parser`; `cargo test -p build-xtask`; `cargo +nightly-2026-09-05 xtask api --check`.
- Verify: the three crates, with `participating_crates_respect_the_file_line_ceiling` (`crates/build-xtask/src/tidy-tests.rs`, line 22) passing with them participating.
- Commit: one commit with the three crates' moves and markers.
- Done when: every `.rs` file in the three crates is at most 500 lines, all three `src/lib.rs` files contain `//! ## Invariants`, the test counts hold, and every listed gate passes.

</step-13>

<step-14>

### Step 14: Split `promptforge-lua`'s `tests.rs` to the ceiling [completed]

- Component: Lua ceiling
- Piece: `src/tests.rs`, the first of two pieces built sequentially. This file alone (3,653 lines at `a05d5cbd`, plus Step 3's tests) fills one sub-agent's read, and the marker waits for the last file, in Step 15.
- Todo: `split-lua-tests`
- Depends on: Steps 2 to 4, 10, and 12, every fix that can edit `promptforge-lua`.
- Read: Technical Design, Structure and Split method; Project Survey.
- Build:
  - Count, and read the crate's test count, first.
  - `crates/promptforge-internal/lua/src/tests.rs` splits by its topic groups, such as the cancellation tests near line 2285 with Step 3's additions, into a `tests/` directory in standard layout with `tests.rs` as the parent. The shared helpers stay in `tests-recording.rs`.
  - No marker yet.
- Tests: none added. The test count after is no lower, and the coder's return states both counts.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-lua --all-targets --all-features -- -D warnings`.
- Verify: `promptforge-lua`.
- Commit: one commit with the move.
- Done when: every file cut from `tests.rs` is at most 500 lines, the test count holds, and the Lua suite passes.

</step-14>

<step-15>

### Step 15: Split the rest of `promptforge-lua` to the ceiling and add its marker [completed]

- Component: Lua ceiling
- Piece: the remaining Lua files, the second piece, built after Step 14, with the marker.
- Todo: `split-lua-rest`
- Depends on: Step 14.
- Read: Technical Design, Structure, Split method, and Enforcement; Project Survey.
- Build, under `crates/promptforge-internal/lua/src/`:
  - Count, and read the crate's test count, first.
  - `vm.rs` (1,457 lines at `a05d5cbd`, less Step 4's deletions), `host.rs` (679), `error-value.rs` (543), and `protocol/parse.rs` (526) split along their existing seams, and `prelude-tests.rs` (539, plus Step 10's test) splits by topic. Any other Lua file the count finds over 500 joins this step.
  - Marker in `lib.rs`: may depend on `promptforge-types`, `promptforge-model-client`, and `promptforge-vfs`.
- Tests: none added. The test count after is no lower, and the coder's return states both counts.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-lua --all-targets --all-features -- -D warnings`; `cargo nextest run --locked -p promptforge-engine --all-features`; under the survey's `RUSTDOCFLAGS` setting, cleared afterwards, `cargo doc --locked --no-deps --all-features -p promptforge-lua`; `cargo test -p build-xtask`; `cargo +nightly-2026-09-05 xtask api --check`.
- Verify: `promptforge-lua`, plus the component test pattern for `promptforge-engine`, which runs every section through the split VM.
- Commit: one commit with the moves and the marker.
- Done when: every `.rs` file in `promptforge-lua` is at most 500 lines, `lib.rs` contains `//! ## Invariants`, the test count holds, and every listed gate passes.

</step-15>

<step-16>

### Step 16: Split the `promptforge-engine` source files to the ceiling [completed]

- Component: Engine ceiling
- Piece: the engine's source files, the first of four pieces built sequentially, each sized for one sub-agent: the source files (3,556 lines at `a05d5cbd`), `exec_flow.rs` (2,641), the scheduler tests (4,237), and the other tests (4,393). The engine's 14,827 oversized lines do not fit one step, and the marker waits for the last piece.
- Todo: `split-engine-source`
- Depends on: Steps 2, 4, 7, 10, 11, and 12, every fix that edits `promptforge-engine`.
- Read: Technical Design, Structure and Split method; Project Survey.
- Build, under `crates/promptforge-internal/engine/src/`:
  - Count, and read the crate's test count, first.
  - `error.rs` (1,092 lines, 860 before its inline tests, plus Step 2's wildcard arm): the tests move out, and the source splits along its seams.
  - `execute/scheduler/tasks.rs` (760), `execute/scheduler.rs` (563), and `test_support/tokio_driver.rs` (554) split along their seams, and `test_support/recording/forward-tests.rs` (587) splits by topic.
  - No marker yet.
- Tests: none added. The test count after is no lower, and the coder's return states both counts.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-engine --all-targets --all-features -- -D warnings`; under the survey's `RUSTDOCFLAGS` setting, cleared afterwards, `cargo doc --locked --no-deps --all-features -p promptforge-engine` and `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`; `cargo +nightly-2026-09-05 xtask api --check`.
- Verify: `promptforge-engine`, including its private-items docs build.
- Commit: one commit with the moves.
- Done when: every file cut from these five is at most 500 lines, the test count holds, and the engine suite passes.

</step-16>

<step-17>

### Step 17: Split `promptforge-engine`'s `suite/exec_flow.rs` to the ceiling [completed]

- Component: Engine ceiling
- Piece: `exec_flow.rs`, the second piece, built after Step 16.
- Todo: `split-engine-exec-flow`
- Depends on: Step 16.
- Read: Technical Design, Structure and Split method; Project Survey.
- Build:
  - Count, and read the crate's test count, first.
  - `crates/promptforge-internal/engine/src/execute/tests/suite/exec_flow.rs` (2,641 lines at `a05d5cbd`, less the comments Step 4 removed) splits by its topic groups.
  - No marker yet.
- Tests: none added. The test count after is no lower, and the coder's return states both counts.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-engine --all-targets --all-features -- -D warnings`.
- Verify: `promptforge-engine`.
- Commit: one commit with the move.
- Done when: every file cut from `exec_flow.rs` is at most 500 lines, the test count holds, and the engine suite passes.

</step-17>

<step-18>

### Step 18: Split `promptforge-engine`'s scheduler tests to the ceiling [completed]

- Component: Engine ceiling
- Piece: the scheduler test files, the third piece, built after Step 17.
- Todo: `split-engine-scheduler-tests`
- Depends on: Step 17.
- Read: Technical Design, Structure and Split method; Project Survey.
- Build, under `crates/promptforge-internal/engine/src/execute/tests/scheduler/`:
  - Count, and read the crate's test count, first.
  - `walk.rs` (1,258 lines at `a05d5cbd`), `failures.rs` (880), `fanout.rs` (854), `live_h1.rs` (723), and `concurrency.rs` (522), with Step 11's `entered` test wherever it landed, split by their topic groups.
  - No marker yet.
- Tests: none added. The test count after is no lower, and the coder's return states both counts.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-engine --all-targets --all-features -- -D warnings`.
- Verify: `promptforge-engine`.
- Commit: one commit with the moves.
- Done when: every scheduler test file is at most 500 lines, the test count holds, and the engine suite passes.

</step-18>

<step-19>

### Step 19: Split the other `promptforge-engine` tests to the ceiling and add its marker [completed]

- Component: Engine ceiling
- Piece: the other test files, the last piece, built after Step 18, with the marker.
- Todo: `split-engine-other-tests`
- Depends on: Step 18.
- Read: Technical Design, Structure, Split method, and Enforcement; Project Survey.
- Build, under `crates/promptforge-internal/engine/src/`:
  - Count, and read the crate's test count, first.
  - Under `execute/tests/`: `model_and_reply.rs` (788 lines at `a05d5cbd`), `tasks.rs` (727), `happens_before.rs` (670), `model_task_notices.rs` (594), `tool_call_arm.rs` (547), `context.rs` (542), and `debug_and_counts.rs` (525) split by their topic groups. Any other engine file the count finds over 500 joins this step.
  - Marker: `//!` lines in `lib.rs` beside `#![doc = include_str!("lib.md")]`: may depend on `promptforge-types`, `promptforge-lua`, `promptforge-parser`, `promptforge-vfs`, and `promptforge-model-client`.
- Tests: none added. The test count after is no lower, and the coder's return states both counts.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-engine --all-targets --all-features -- -D warnings`; under the survey's `RUSTDOCFLAGS` setting, cleared afterwards, `cargo doc --locked --no-deps --all-features -p promptforge-engine` and `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`; `cargo test -p build-xtask`.
- Verify: `promptforge-engine`, including its private-items docs build.
- Commit: one commit with the moves and the marker.
- Done when: every `.rs` file in `promptforge-engine` is at most 500 lines, `lib.rs` contains `//! ## Invariants`, the test count holds, and every listed gate passes.

</step-19>

<step-20>

### Step 20: Split `promptforge-vfs`'s `handle.rs` to the ceiling [completed]

- Component: VFS ceiling
- Piece: `handle.rs`, the first of two pieces built sequentially. This file alone (3,825 lines at `a05d5cbd`) fills one sub-agent's read, and the marker waits for the last file, in Step 21.
- Todo: `split-vfs-handle`
- Depends on: Steps 5, 6, 7, and 12, every fix that edits `promptforge-vfs`.
- Read: Technical Design, Structure and Split method; Project Survey.
- Build:
  - Count, and read the crate's test count, first.
  - `crates/promptforge-internal/vfs/src/handle.rs`, with Step 6's claims checks and without Step 7's grep code: the source (2,577 lines before its inline tests at `a05d5cbd`) splits along its scope, claims-ledger, handle, store-view, and forwarding-wrapper seams into a `handle/` directory in standard layout, with `handle.rs` as the parent. The inline tests (from line 2578) move out and split by topic, the claims tests among them. The ledger's internals take `pub(super)` or `pub(crate)`, private to the crate but visible across the new modules.
  - No marker yet.
- Tests: none added. The test count after is no lower, and the coder's return states both counts.
  - Gate commands: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-vfs --all-targets --all-features -- -D warnings`; `cargo nextest run --locked -p promptforge-engine --all-features`; under the survey's `RUSTDOCFLAGS` setting, cleared afterwards, `cargo doc --locked --no-deps --all-features -p promptforge-vfs`; `cargo +nightly-2026-09-05 xtask api --check`.
- Verify: `promptforge-vfs`, plus the component test pattern for `promptforge-engine`, which runs every store operation through the split ledger.
- Commit: one commit with the move.
- Done when: every file cut from `handle.rs` is at most 500 lines, the test count holds, and the VFS and engine suites pass.

</step-20>

<step-21>

### Step 21: Split the rest of `promptforge-vfs`, add its marker, and pass the exit gate [completed]

- Component: VFS ceiling
- Piece: the remaining VFS files, the second piece, built after Step 20, with the marker and the full exit gate.
- Todo: `split-vfs-rest`
- Depends on: Step 20, and every earlier step for the exit gate.
- Read: Technical Design, Structure, Split method, Enforcement, and File and public API changes; Project Survey.
- Build, under `crates/promptforge-internal/vfs/src/`:
  - Count, and read the crate's test count, first.
  - `host.rs` (1,345 lines at `a05d5cbd`, 706 before its inline tests, plus Step 5's tests): the tests move out and split by topic, and the source splits along its seams.
  - `router.rs` (977, 488 before its tests), `memory.rs` (886, 460), `traits.rs` (810, 476), and `detail.rs` (620, 110), less Step 7's deletions: each moves its inline tests to a `-tests.rs` sibling, which splits again by topic if it is over 500. Any other VFS file the count finds over 500, such as `lib.rs` after Step 12, joins this step.
  - Marker in `lib.rs`: std only, with no dependencies, as its manifest test enforces.
- Tests: none added. The test count after is no lower, and the coder's return states both counts.
  - Gate commands, beyond the full run: `rg -l '^//! ## Invariants' crates/promptforge-internal/engine/src/lib.rs crates/promptforge-internal/lua/src/lib.rs crates/promptforge-internal/parser/src/lib.rs crates/promptforge-internal/vfs/src/lib.rs crates/promptforge-internal/model-client/src/lib.rs crates/promptforge-internal/types/src/lib.rs` lists all six files; `git diff a05d5cbd -- crates/promptforge/public-api.txt` holds exactly the changes File and public API changes names.
- Verify: FULL scope, every Exit criteria command, then the Success criteria: all six crates' `src/lib.rs` contain `//! ## Invariants`; `cargo test -p build-xtask` passes with `participating_crates_respect_the_file_line_ceiling` covering all six; and `git diff a05d5cbd -- crates/promptforge/public-api.txt` holds exactly the changes File and public API changes names.
- Commit: one commit with the moves and the marker.
- Done when: every `.rs` file in the six crates is at most 500 lines, every Exit criteria command passes, and the cumulative listing diff matches.

</step-21>

</execution-plan>
