---
name: Cache sidecar ordering
overview: "Remove the debt the upstream/master..HEAD work added, plus the cheap leftovers: publish the model cache sidecar before the blob (D1-1), make every non-conforming Lua fence a hard parse error (D1-14, D1-13), drop the last slot-era absence test (D1-3), and fix the stale 'offers every tool' comments (D1-26)."
todos:
  - id: cache-sidecar
    content: "D1-1: publish the cache sidecar before the blob in both writers, with the two sidecar-fault regression tests"
    status: completed
  - id: strict-fences
    content: "D1-14, D1-13: whole-body strict Lua fence check, flipped and new parser tests, two facade fixtures, and the fence docs"
    status: completed
  - id: leftover-traces
    content: "D1-3, D1-26: drop the slot-era nil checks and rewrite the four stale 'offers every' comments"
    status: completed
isProject: false
---

# Cache Sidecar Ordering, Strict Lua Fences, and Leftover Traces

<product-contract>

## Product Requirements

- Scope and target work: `upstream/master` (7476ed81d) to `master` HEAD (fdb20bad5). That is 17 commits under two plans (`vibe/2026-10-14-1-no-trace-fence.md`, `vibe/2026-10-14-2-facade-suite-dedup.md`) plus the rustls lockfile bump. The debt added comes from b45fe12f2. The user widened the scope to one exposed item (D1-3) and three rejected candidates (D1-13, D1-14, D1-26).
- Cleanup goals:
  - Cache: a model blob that `ArtifactStore` or `BlobCache` publishes into `models/<key>/<filename>` always has a complete `<file>.meta.json` sidecar. The cache listing (`GET /v1/cache`), cache deletion, and the config UI's Local Models file status then never disagree with provisioning.
  - Fences: a Lua fence is spelled exactly one way. Every other Lua-looking fence or line fails the parse. The only exception is text inside the body of another fenced code block.
  - Traces: no removed slot-era tool names remain in the Lua tests, and no comment claims the Engine offers every catalog tool.
- Non-goals:
  - hash-on-hit repair for sidecar-less blobs, or repairing blobs already on disk
  - changing the sidecar JSON format
  - changing which labels count as Lua-like (a label starting with `lua` in any letter case)
  - every candidate still rejected (see the Debt Inventory)
- Success criteria:
  - every regression test named in the Testing Plan fails at fdb20bad5 on its named assertion and passes after the change
  - the tests listed as unchanged stay green without edits
  - no shipped prompt, fixture, or Workshop agent file needs an edit. A search at fdb20bad5 found no non-conforming Lua fence outside `vibe/` plan files, which are never parsed as prompts, and the intended `unrecognized-lua-label.md` fixture.

## Functional Specification

### Strict Lua fence rule

- A Lua fence is an opening line, then body lines, then a closing line. The opening line is exactly three backticks followed by `lua`, and the closing line is exactly three backticks. Below, "the opener" means that exact opening line and "the closer" means that exact closing line:

~~~text
the opener:  ```lua
the closer:  ```
~~~

- "Exactly" means nothing before or after on the line. The line terminator is not part of the line, so LF and CRLF files both work.
- The rule covers the whole body after the frontmatter (the preface before the H1, the H1, and every section) and runs before any Lua compiles.
- Exception: lines inside the body of another fenced code block are never inspected. That is how a document shows a Lua fence as an example. This parses as prose, because the four-backtick fence encloses the inner one:

~~~markdown
````
some crap example
```lua caca
blahblah
```
more
````
~~~

- Everything else fails with `ParseErrorKind::Fence`, located at the offending line. The first violation in source order wins. Within one fence, the label rule is checked before the opener rule.
  - Label (unchanged): a fence whose label starts with `lua` in any letter case but is not exactly `lua`, such as `Lua`, `luau`, or `lua extra`.
  - Opener: a fence labeled exactly `lua` whose opening line is not the opener. Covered shapes: a trailing space or tab, one to three spaces of indent, a space between the backticks and the label, tildes, four or more backticks, and a fence inside a block quote or list item.
  - Closer: a fence that starts with the opener but whose closing line is not the closer. Covered shapes: a trailing space or tab, indent, and four or more backticks. An unclosed fence keeps its existing per-section "is not closed" error and its location wording.
  - Lookalike: a line outside every fenced code block whose text, after leading spaces and tabs, is three or more backticks or tildes, then optional spaces or tabs, then `lua` in any letter case. These are places where the Markdown parser reports no fence: an indented code block (four or more spaces) and a raw HTML block.
- Messages. The label message is unchanged, and the opener and lookalike rules share one message:

~~~text
label:              unrecognized fence label `lua extra`; the only Lua fence label is `lua`
opener, lookalike:  `lua` fence opening line must be exactly ```lua
closer:             `lua` fence closing line must be exactly ```
~~~

### Debt Inventory

- Debt added:
  - D1-1 (introduced, b45fe12f2).
    - Evidence: b45fe12f2 removed `blob_meta_matches`, the unpinned-hit repair in [crates/gateway/local/src/artifacts/blob.rs](crates/gateway/local/src/artifacts/blob.rs), and the test `artifact_store_migrates_an_unpinned_cache_hit_into_the_listing`. At the endpoint, an unpinned hit returns `Ok(())` at lines 43-46 without reading or writing the sidecar.
    - Both download paths publish first and write the sidecar second: `blob.rs` lines 96-108 (`rename_confined`, then `write_blob_meta`), and [crates/gateway/local/src/cache.rs](crates/gateway/local/src/cache.rs) `BlobCache::download_to_cache` lines 264-283 (`rename_confined`, then `write_synced`). `write_synced` ([crates/gateway/local/src/artifacts/confine.rs](crates/gateway/local/src/artifacts/confine.rs) line 355) is an in-place truncating write.
    - Impact: if the sidecar write fails or is cut short after the rename, every later unpinned `ensure_model` treats the blob as a hit forever. Meanwhile `BlobCache::list` skips it, `BlobCache::remove` cannot find it, and the config UI shows an in-use model as "Not downloaded" with no delete control. This contradicts the `cache.rs` module doc ("a cache-API download is a provisioning cache hit for the same URL, and vice versa"; "Each published blob gets a `<file>.meta.json` sidecar").
    - Reversal cost: low. Crate-private code, no format change.
    - Target state: neither download path publishes a blob before its sidecar is complete on disk.
- Fixed anyway at the user's request:
  - D1-14 (was residual-but-acceptable; the user chose a hard error).
    - At fdb20bad5, a fence labeled exactly `lua` with a non-exact opening line silently stays prose. `exact_lua_openings` ([crates/runvil-internal/parser/src/fence.rs](crates/runvil-internal/parser/src/fence.rs) line 148) requires the exact line, and `first_unrecognized_lua_label` (line 156) flags only labels other than `lua`. The Lua source reaches the model as prompt text.
    - A non-exact closing line stays in the Lua source up to the next line that is exactly the closer. That swallows the prose in between and surfaces as a confusing Lua compile error or a "not closed exactly" error.
    - Lua-looking lines inside indented code blocks and HTML blocks are also silently prose.
    - Two tests pin the silent behavior: `h1_shape_near_misses_of_an_exact_lua_label_remain_prose` (`src/tests/h1_fences.rs` line 96) and `section_fence_markers_must_be_exact` (`src/tests/section_fences.rs` line 96).
    - Target state: the strict rule above.
  - D1-13 (was weak/speculative). The `fence.rs` docs at lines 125 and 154 say "top-level", but the Markdown parser also reports fences inside block quotes and list items, and the label rule fires there. Fixed by the D1-14 doc rewrite.
  - D1-3 (exposed). [crates/runvil-internal/lua/src/tools/tests-offering.rs](crates/runvil-internal/lua/src/tools/tests-offering.rs) still has `the_slot_era_functions_are_not_installed` (line 229), which asserts that `tools.add`, `tools.always`, `tools.offered`, and `tools.add_local` are nil. Its module doc (line 3) also mentions "the removed slot-era functions". This contradicts the no-trace-fence rule ("Not in tests"). 88839e79c deleted five absence tests of the same kind and left this one.
  - D1-26 (was residual-but-acceptable). Four comments still say the Engine offers every catalog tool or descriptor. That contradicts the `RunContext::tools` doc 6cb799027 corrected ("The catalog lists every tool the run can use: Lua calls one by id, and a section offers one to its model with `tools.offer`"). The comments are at:
    - `crates/runvil-internal/engine/src/tools.rs` line 3
    - `crates/runvil-internal/types/src/tools/registry.rs` line 13
    - `crates/runvil-internal/types/src/tools/descriptor.rs` line 17
    - `crates/runvil-internal/engine/src/execute/tests/context.rs` line 147
- Cheap fixes: none beyond the above.
- Rejected candidates still out of scope (28):
  - residual-but-acceptable (15):
    - Intended removals under no-trace-fence Step 4: a `[workshop]` config section now fails, `profiles/` is ignored, the v1 sidecar body is ignored, the legacy rotation marker and legacy temp name are no longer handled, and a stored Render Whitespace `true` falls back to the default.
    - Recorded user decisions: the lint and no-lucide guard removals, `auto_traits.rs` moving to the `public-api.txt` CI check, and vfs dev-dependencies.
    - The intended label rule.
    - Test-harness choices: string tool ids and a fixed run name.
    - Named deferrals: `Effect::Timer` f64 seconds, and the remaining `lua_over`, `flow_prompt!`, and `args_prompt!` copies.
  - weak/speculative (4): line trimming in the test `prompt` builder, the engine suite reading workspace `prompts/`, the extra `getrandom` edge in the rustls bump, and the stall-check gap for decided runs in the test driver.
  - false (8): CSS alias references, `HardBreak` consumers, `config-version` validation, surviving clone blocks, coverage lost from the 13 deleted facade tests and from the sweep's absence tests, unprepared-run coverage, and assertions dropped in the harness migration.
  - unrelated pre-existing (1): the Workshop `ServerConfig` has no `deny_unknown_fields`.

</product-contract>
<implementation-contract>

## Technical Design

### Cache sidecar ordering (D1-1)

- Publish-order invariant for both writers into the shared `models/<key>/<filename>` slot:
  1. Remove whatever sits at the destination.
  2. Write the complete sidecar at `<destination>.meta.json`, with the size measured from the staging `.part` file.
  3. `rename_confined` the staging file to the destination.
  4. For pinned assets only, write the `.verified` marker best-effort, as today.
- Removal must come before the sidecar write. Otherwise a crash between steps leaves an old blob beside a sidecar describing new content, and `cached` serves the old bytes under the new digest.
  - In `ArtifactStore::ensure_blob_with_progress`, the destination is already absent at the download step (lines 43-67), so only the sidecar write moves above `rename_confined`.
  - In `BlobCache::download_to_cache`, keep `remove_cache_entry(&self.root, &destination)` first, then write the sidecar, then rename.
- `crate::cache::write_blob_meta` becomes the only code that builds and writes a `BlobMeta`:
  - It measures a file passed separately from the blob whose sidecar path it derives, and it returns the size it recorded.
  - Delete the inline `BlobMeta` write in `download_to_cache`.
  - The pinned verified-hit call at `blob.rs` line 51 keeps measuring the destination.
  - The function is `pub(crate)`, so its signature is free to change.
- After the change, a sidecar write failure returns the error with no blob published. The completed `.part` has no provenance marker (`artifacts/download.rs` line 392), so the next attempt restarts from zero. A sidecar without a blob is skipped by `BlobCache::list`, is not a hit, and is overwritten by the next download.

### Strict fence check (D1-14, D1-13)

- In `crates/runvil-internal/parser/src/fence.rs`, replace `first_unrecognized_lua_label` with one whole-body check, for example `first_fence_violation(body) -> Option<(Range<usize>, FenceViolation)>`. `FenceViolation` has three variants: `Label(String)`, `Opener`, and `Closer`. At line 133, `parse.rs` maps each variant to its message and returns `Error::parse_at(ParseErrorKind::Fence, line_range, message)`, as it does today for labels.
- One pulldown-cmark pass over the body yields every fenced code block with its opening line, its label, and its extent.
  - Take the closing line from the block's own extent in the event stream, not from a separate scan, so the check and the Markdown parser always agree on where a fence ends.
  - A block whose extent ends without a closing-fence line is unclosed and is left to `split_section_blocks`.
- The lookalike scan walks the body's lines and skips every line inside a reported fenced block's extent. That skip is the nested-fence exception, and it also keeps opener lines from being checked twice.
- `split_section_blocks` keeps using exact openers and `extract_exact_fence`. Once the whole-body check passes, the first closer it finds is the line the Markdown parser closes on, so the `opening < pos` branch ("is not closed exactly") can no longer be reached. Change that branch to the existing internal `Structure` error ("internal section fence classification mismatch"), so no author-facing message describes a state the check rules out.
- Unchanged: the label rule and its message, the set of Lua-like labels, frontmatter handling, and section splitting.

### Fence docs (D1-14, D1-13)

- `Prompt::parse`, the `Fence` bullet (`parse.rs` lines 40-44), and `ParseErrorKind::Fence` (`error.rs` lines 175-176): state the strict rule and the nested-fence exception.
- In `fence.rs`:
  - the module doc (lines 1-8)
  - `fence_openings`: say it yields every fence the Markdown parser reports, including fences inside block quotes and list items, and never a fence inside another code block's body (D1-13)
  - the new check's doc
  - `split_section_blocks`: drop "Near-miss fence forms stay inside prose"
  - `extract_exact_fence`: drop "Near-miss closing markers are part of the source"

### Leftover traces (D1-3, D1-26)

- `tests-offering.rs`:
  - Remove "and the removed slot-era functions" from the module doc.
  - In `the_slot_era_functions_are_not_installed`, drop the four `== nil` values from the Lua chunk and the result tuple, and keep the check that the six `tools` entries are functions.
  - Rename it, for example `the_tools_namespace_installs_its_six_functions`.
- Rewrite the four comments to say every catalog tool is available to the prompt's Lua, which calls one by id, and that a section offers one to its model with `tools.offer`. In `context.rs`, say that the fixture Plugin's tools are in the run's catalog.

</implementation-contract>
<verification-contract>

## Testing Plan

### D1-1, in [crates/gateway/local/src/cache/tests.rs](crates/gateway/local/src/cache/tests.rs)

- The fault: after constructing the store or cache, `fs::create_dir_all(meta_path(&destination))`, so `File::create` in `write_synced` fails.
- `artifact_store_publishes_no_blob_whose_sidecar_write_failed`
  - Setup: `FakeServer::new(body)` and `ArtifactStore::new(temp.path())`. Get the destination from `BlobCache::new(temp.path())?.destination(&url)`.
  - The first `ensure_model(&url, None)` errors. Then `assert!(!destination.exists())`, which fails at fdb20bad5.
  - Remove the directory and call `ensure_model(&url, None)` again. `assert_eq!(server.requests(), 2)` fails at fdb20bad5, where the count is 1.
  - `BlobCache::list()` equals one `CacheEntry` with `hex_sha256(body)` and `body.len()`. This fails at fdb20bad5, where the list is empty.
- `download_to_cache_publishes_no_blob_whose_sidecar_write_failed`
  - Same fault. `download_to_cache(&url, None, ..)` errors, and `assert!(!destination.exists())` fails at fdb20bad5.
  - Remove the directory and retry. The returned blob's sidecar parses, and `server.requests() == 2`.
- Unchanged and green: `download_to_cache_downloads_verifies_and_writes_sidecar`, `download_to_cache_rejects_digest_mismatch_and_keeps_the_partial`, `download_to_cache_resumes_an_interrupted_partial`, `sidecar_less_blob_is_not_a_hit_and_is_replaced`, `corrupt_sidecar_is_skipped_and_not_a_hit`, `concurrent_publishers_converge_on_one_download`, `mismatched_pin_against_sidecar_forces_redownload`, `artifact_store_downloads_appear_in_the_cache_listing`, and `tests-listing.rs`.

### D1-14 and D1-13, in `crates/runvil-internal/parser/src/tests/`

- Shape test: flip `h1_shape_near_misses_of_an_exact_lua_label_remain_prose` (`h1_fences.rs`) and `section_fence_markers_must_be_exact` (`section_fences.rs`) into error tests.
  - Shapes:
    - the opener followed by a trailing space, and followed by a trailing tab
    - three backticks, a space, then `lua`
    - the opener indented by one space, and by three spaces
    - `~~~lua`, closed by `~~~`
    - four backticks then `lua`, closed by four backticks
    - the opener inside a block quote, and inside a list item
  - Each must fail with kind `Fence` and the opener message, in the H1, in a section, and in the preface.
  - At fdb20bad5 each one parses, so `expect_err` fails.
- Closer test: update `non_exact_section_closing_before_another_lua_fence_is_a_parse_error` (`section_fences.rs` line 140) to assert kind `Fence` and the closer message. Keep its three bad closers: the closer plus a trailing space, the closer indented by two spaces, and four backticks.
  - At fdb20bad5 the message is "section `S` `lua` fence is not closed exactly", so the message assertion fails.
  - Add a case with no later Lua fence, where the bad closer is followed by prose and then a closed `python` fence. At fdb20bad5 this fails with kind `Lua`, because the Lua source swallowed the prose, so the kind assertion fails.
- Lookalike test: a section holding the opener indented by four spaces after a blank line, and a `<div>` HTML block whose next line is the opener, with `</div>` after the body. Each must fail with the opener message. At fdb20bad5 both parse as prose.
- Ordering test: an exact H1 Lua block followed by a section with an opener violation reports `PARSE_STARTED` then `PARSE_FAILED`, with no Lua compilation events. This mirrors `unrecognized_lua_label_fails_before_any_lua_compiles`. At fdb20bad5 the parse succeeds.
- Exception guard: the user's example verbatim, plus a four-backtick fence holding the opener with a trailing space and the opener indented by four spaces. All must parse with the inner text in prose. This passes at fdb20bad5 by design: it guards the exception rather than proving a fix.
- Facade fixtures in `crates/runvil/tests/prompts/invalid/`:
  - Add `lua-fence-opener.md`, whose Lua fence opens with `~~~lua` (a trailing-space fixture would not survive editors that strip whitespace), and `lua-fence-closer.md`, whose Lua fence starts with the opener and closes with four backticks.
  - Add both to `INVALID_FIXTURES` in `crates/runvil/tests/suite/parsing.rs` with kind `Fence` and a fragment of each message.
  - At fdb20bad5 the opener fixture parses, so the test reports "unexpectedly parsed". The closer fixture fails with the old "is not closed" message, so the fragment check fails.
- Unchanged and green:
  - `h1_label_near_misses_fail_whatever_the_opener_shape` (the label rule still wins on four-backtick and indented openers)
  - `label_near_misses_inside_longer_fences_remain_prose` and `section_markers_inside_longer_fences_remain_prose`
  - `fence_labels_not_starting_with_lua_stay_in_prose` and `non_lua_fence_stays_in_prose`
  - `unclosed_reserved_section_fences_are_location_errors`
  - the CRLF tests (`section_compiles_prologue_and_epilog_around_prose`, `unrecognized_lua_label_in_the_h1_fails_the_parse`)
  - `line_mapping.rs`
  - the engine suite's `every_prompt_under_parses` over the shipped `prompts/` tree

### D1-3 and D1-26

- `rg -n -i "slot.era|tools\.(add|always|offered|add_local)\b" crates/runvil-internal/lua` returns nothing, and the renamed test passes.
- `rg -n "offers every" crates/runvil-internal` returns nothing.

### Exit checks

- `cargo nextest run -p gateway-local -p runvil-parser -p runvil-lua -p runvil-engine -p runvil`, the workspace clippy and fmt gates CI runs, and the pre-push hook.
- No structural ratchet is added.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences:
  - D1-1: publish the sidecar before the blob, rather than restoring a repair on the unpinned hit. This closes the failed-write and truncation windows for both writers, keeps the rule against migrating old caches, and never re-hashes a multi-GB blob. The cost: a failed sidecar write now means a full re-download on the next attempt.
  - D1-1: route both writers through `write_blob_meta`, so the publish paths cannot drift apart.
  - D1-14: take fence extents from the Markdown parser's event stream rather than from a second scanner, so the question of where a fence ends has one answer.
  - D1-14: turn the unreachable "not closed exactly" branch into an internal invariant error instead of leaving author-facing text behind.
  - D1-3: keep the positive half of the test, because it pins the installed `tools` surface.
- Operator constraint: keep the step count minimal. The work lands as exactly three commits, one per behavior: cache sidecar ordering, strict Lua fences, and leftover traces.
- User-resolved architecture choices:
  - D1-14: every non-conforming Lua fence is a hard error, maximum strict. In the user's words, the opener "or nothing". The only exception is text inside the body of another fenced code block, such as a four-backtick example. This tightens the public `Prompt::parse` contract and reverses the no-trace-fence plan's decision that shape near-misses stay prose.
  - Applied from that rule, not asked separately: closing lines must be exact; fences inside block quotes and list items fail; and Lua-looking lines inside indented code blocks and HTML blocks fail.
- Rejected alternatives:
  - D1-1:
    - Hash-on-hit repair. It reads as the removed migration code and costs a full hash pass.
    - An atomic `write_synced` alone. It closes only the truncation window.
    - Reordering only `ArtifactStore`. A sidecar-less blob that `BlobCache` publishes is also an unpinned `ArtifactStore` hit.
  - D1-14:
    - Accepting near-miss openers as Lua blocks. The user chose one spelling instead.
    - Exempting indented code blocks and HTML blocks the way nested fences are exempt. The user named nested fences as the only exception.
    - A line scan alone. It cannot see container prefixes such as `> ` or `- `, and it would disagree with the Markdown parser about where fences end.
- Assumptions:
  - Both cache components resolve the same slot for one URL (`artifacts.rs` lines 185-190 and `BlobCache::destination`).
  - `File::create` on a directory fails on both Windows and Unix.
  - CRLF files keep working, because the terminator is not part of the line.
- Risks:
  - Blobs that a build at or before fdb20bad5 already left without a sidecar stay unlisted until they are re-downloaded or removed by hand. This is accepted under the no-trace-fence rule that stale files from older builds are accepted.
  - A Lua-looking line in an indented code block that is itself inside a block quote or list item (for example a `>` marker, five spaces, then the opener) is not caught, because the lookalike scan strips only leading spaces and tabs.
  - A prose line that starts with an inline code span delimited by three backticks, such as one wrapping `lua x`, now fails as a lookalike. The author can reword it.
  - Prompts written outside this repository against the old rule may stop parsing. The error names the line and the exact form required.

### Deferred and Out of Scope

- Repair of sidecar-less blobs already on disk. Revisit if a provisioned model is reported as showing "Not downloaded".
- Merging the remaining `flow_prompt!`, `args_prompt!`, and `lua_over` copies. Both design records defer this to the internal-crates test cleanup.
- `Effect::Timer` keeping `f64` seconds. Deferred to the rulebook audit.
- The Workshop `ServerConfig` lacking `deny_unknown_fields`. Unrelated pre-existing behavior; revisit if typo detection in `[server]` is wanted.
- The other 28 rejected candidates, listed in the Debt Inventory.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the workspace's only default member). Crate build scripts bundle the UIs, so run `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first, as CI does. The desktop app is a separate, explicit build: `cargo workshop` (the `build-workshop` alias, which builds the gateway, stages it as the Tauri sidecar, builds Workshop, and removes the staged sidecar), or `cargo build --locked -p workshop` after `node tools/stage-gateway-sidecar.mjs stage --target <triple> --source <gateway binary>`.
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <test-name-filter>`, or `-E 'test(<name>)'` for a nextest filterset; drop `--all-features` when the package is `workshop`, `workshop-server`, or `workshop-server-api`. The packages this plan touches are `gateway-local` (`ArtifactStore` in `src/artifacts.rs`, `BlobCache` in `src/cache.rs`), `runvil-parser` (`src/fence.rs`), and `runvil` (facade suite binary `suite`, prompt fixtures under `tests/prompts/invalid/`). One Workshop UI test file: `node --test crates/workshop/ui/test/<name>.mjs`.
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features` (again without `--all-features` for `workshop`, `workshop-server`, `workshop-server-api`). Workshop UI packages: `npm test --workspace ui` from `crates/workshop`. Gateway config UI: `npm test` from `crates/gateway/config-ui/ui`. Node tool scripts: `node --test tools/<name>.test.mjs`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. CI also runs `npm test --workspaces --if-present` in `crates/workshop` and `npm test` in `crates/gateway/config-ui/ui`.
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`. Companion gates: `cargo check -p gateway --no-default-features` (headless build shape), `cargo test -p build-xtask` (boundary and structural checks, including the doctest ban and the `test-support` leak guard), `node --test crates/workshop/ui/test/docs-claims.mjs` (wording and rulebook scan), `cargo +<pinned nightly> xtask api --check` (facade surface, nightly named in `crates/build-xtask/src/api/toolchain.rs`), `npm run typecheck --workspaces --if-present` in `crates/workshop`, and `cargo deny check` (CI also runs `cargo audit`). Never run a standalone `cargo check --workspace` beside clippy.
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`; the pre-commit hook runs it).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, plus the facade build with default features: `RUSTDOCFLAGS="-D warnings" cargo doc -p runvil --no-deps`. CI also documents `harness` with default features and `runvil-engine` and `workshop-server` with `--document-private-items`.
- Test placement and naming conventions: Unit tests take three forms, all common. The most common (about 180 modules) is a sibling file `<module>-tests.rs` wired at the end of the module with `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;`; a large tests module splits further into `tests-<label>.rs` siblings (for example `gateway-local`'s `cache/tests.rs` with `tests-listing.rs`). About 140 files keep an inline `#[cfg(test)] mod tests { }`. About 60 use `#[cfg(test)] mod tests;` with a `tests.rs` or a `src/**/tests/` directory, such as `gateway-local`'s `src/artifacts/tests/` and `runvil-parser`'s `src/tests/`. Integration tests compile as one binary per crate at `tests/it/main.rs` (most crates) or `tests/suite/main.rs` (`runvil`, `harness`), with one `mod` file per area plus a `support` module. Standalone `tests/<name>.rs` binaries exist only in `build-workshop`, `gateway-cloud-providers`, `gateway-stt-engine`, and `gateway-stt-backend-whisper`, whose `native_*.rs` files need native whisper assets and are `#[ignore]`d. Test data sits in `tests/fixtures/` (`gateway-cloud-providers`, `workshop-protocol`, `workshop-server`), shared helpers in `workshop-server`'s `tests/common/`, and Engine prompt fixtures in `crates/runvil/tests/prompts/`. Test functions are snake_case sentences that state the behavior, such as `download_to_cache_rejects_digest_mismatch_and_keeps_the_partial`; Miri-run tests carry a `miri_` prefix. Workshop UI tests are `node:test` files at `crates/workshop/ui/test/<kebab-name>.mjs`; gateway config UI tests sit beside their sources as `src/**/*.test.mjs`; Node tool scripts sit beside `tools/<name>.test.mjs`.
- Directory map: `crates/` holds all code. Its top level is the public and shared layer: `runvil` (the Engine's public API facade), `runvil-plugin` (the Plugin API), `harness` and `harness-gateway-client`, the linked Plugins `plugin-web`, `plugin-mcp`, and `plugin-user-input`, the gateway vocabulary and discovery crates `gateway-api-types` and `gateway-api-discovery`, `shared-error-source` and `shared-loopback`, `shared-ui` (a TypeScript and CSS package, not a Rust crate), and the `build-*` tooling (`build-xtask` behind `cargo xtask`, `build-workshop` behind `cargo workshop`, plus `build-ui`, `build-user-guide`, `build-llama-cuda`, `build-ceiling`). Four manifestless containers hold each family's private crates: `crates/runvil-internal/` (Engine: `types`, `parser`, `lua`, `engine`, `vfs`, `model-client`), `crates/harness-internal/` (`runner`), `crates/gateway/` (the `app` binary, `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `cloud-providers`, `web-search`, and `stt/` with `api`, `engine`, `backend-whisper`, `whisper-ffi`), and `crates/workshop/` (the Tauri `desktop` app, `server`, `server-api`, the tier crates, and the npm workspace members `ui`, `look`, `platform`). Outside `crates/`: `guide/` is the user guide source (books for gateway, language, and workshop, plus `chrome` and `landing`); `prompts/` holds sample prompt programs; `vibe/` holds dated implementation plans; `tools/` holds Node scripts for staging the gateway sidecar and a live TTS check; `images/` holds README and site art; `.github/workflows/` holds CI (`ci.yml`), release, nightly, Miri, site, and installer workflows; `.githooks/` holds pre-commit (fmt) and pre-push (wording scan, headless gateway check, clippy, cargo deny); `.cargo/config.toml` sets rust-lld and the static CRT on Windows plus the `xtask` and `workshop` aliases; `.config/nextest.toml` sets timeouts and the heavy STT test group; `.cursor/rules/` holds the Workshop architecture and SPA rules; root `deny.toml`, `clippy.toml`, `rustfmt.toml`, and `dist-workspace.toml` (cargo-dist) configure the gates and releases. `local/`, `target/`, and `target-msrv/` are gitignored, and `cabinet/` holds no tracked files.
- Component boundaries: Engine: the `runvil` facade depends on all six `runvil-internal` crates; inside, `runvil-engine` depends on the other five, `runvil-parser` depends on `runvil-lua` and `runvil-types`, `runvil-lua` on `runvil-model-client`, `runvil-types`, and `runvil-vfs`, and `runvil-types` and `runvil-vfs` are leaves. Plugin API: `runvil-plugin` depends on `runvil-types` and `runvil-vfs`, and each linked Plugin depends only on `runvil-plugin`. Harness: `harness` depends on `harness-runner`, `runvil`, and `runvil-plugin`; `harness-runner` depends on `runvil` and `runvil-plugin`; `harness-gateway-client` depends on `harness`, `plugin-web`, and `runvil`. Gateway: the `gateway` app sits on top of its family crates; `gateway-local` (the model cache and artifact store) depends on `gateway-config`, `gateway-progress`, `gateway-protocol`, `gateway-routing`, and `shared-error-source`, and `gateway-stt` depends on `gateway-local`; the family reaches outside only through `gateway-api-types`, `gateway-api-discovery`, and the `shared-*` crates, and nothing in it depends on the Engine or Harness. Within STT, `gateway-stt` depends on `gateway-stt-engine` and `gateway-stt-backend-whisper`, which depends on `gateway-whisper-ffi`, the subsystem's only `unsafe` code. Workshop: tiers flow server, then features, then services, then vocabulary; `workshop-server` is the composition root and depends on every Workshop crate plus `harness`, `harness-gateway-client`, the Plugins, and `runvil`; the desktop package `workshop` depends only on `workshop-server-api` and `gateway-api-discovery`; `workshop-protocol` and `workshop-support` are leaves and `workshop-registry` depends only on `workshop-protocol`; Workshop reaches the gateway only through `gateway-api-types` and `gateway-api-discovery`. The `build-*` crates have no workspace dependencies. `cargo test -p build-xtask` enforces the product and container boundaries, the Workshop tier graph, the `## Invariants` marker, and lint inheritance.
- Conventions summary: Rust edition 2024 on the stable toolchain with resolver 3 and every dependency declared once in `[workspace.dependencies]`, each non-obvious choice explained by a comment. Workspace lints deny clippy `all` and `pedantic`, `unwrap_used` and `expect_used` (allowed in tests by `clippy.toml`), bare `allow` attributes (use `#[expect(..., reason = "...")]`), and `unsafe_code`; `missing_docs`, `missing_debug_implementations`, and `unreachable_pub` warn and the gate denies warnings; `clippy.toml` bans process-global installers outside binary entry points. Doc comments hold no compiled code blocks (examples go in tests), and a `test-support` feature is enabled only from `[dev-dependencies]`. Source directories are flat: a subdirectory needs at least three files, otherwise modules sit beside the parent as `parent-label.rs` wired with `#[path]`. Each Workshop crate's `lib.rs` is a facade whose `//!` doc carries a `## Invariants` section listing allowed dependencies. Engine, Harness, Host, and Plugin are capitalized defined terms, and Engine crates say "the caller" instead of naming the Host. Comments state only non-obvious constraints, and workarounds cite their upstream issue URL. Error messages are written for model consumption and name required versus actual. Behavior changes ship with tests in the same change. JSON reaching a recorder or replay round-trips exactly with sorted keys. Cargo features gate real build constraints, not product shape. Workshop UI CSS uses design tokens only, and persisted UI values go through the `ui-storage` adapter. Text files are LF (`* text=auto eol=lf`, CRLF only for `.ps1` and `.bat`), with the UI trees pinned to LF explicitly. Commit subjects are imperative sentence case, and a finished plan lands as a `Close plan: <slug>` commit.

</project-survey>
<execution-plan>

## Execution Instructions

Three components, three steps, one commit per step, in the order below. The Decision Record's operator constraint fixes this shape: exactly three commits, one per behavior, none split further. Each commit holds its code and its tests, compiles, passes the suites it touches, and leaves nothing unused, since clippy runs with warnings denied. Paths are relative to the repository root, `c:\Users\Vinnie\cursor\runvil`. The three components touch disjoint files, so each step's red-first check runs against the same code fdb20bad5 had for that behavior. The exit checks run in Step 3.

- Components, in dependency order:
  1. Cache sidecar ordering (Step 1), in `gateway-local`. First, because D1-1 is the only debt the target range introduced, and it shares no crate with the other two.
  2. Strict Lua fences (Step 2), in `runvil-parser` plus the `runvil` facade fixtures. Second, because it is the largest change and tightens the public `Prompt::parse` contract. Nothing in Step 3 depends on it, and landing it before Step 3 puts the strict parser under the final exit checks.
  3. Leftover traces (Step 3), in `runvil-lua`, `runvil-engine`, and `runvil-types`. Last, because it is test and comment edits only, so the final commit can carry the exit checks across every package the plan touches.
- Pieces and construction:
  - Cache sidecar ordering: one piece, with both writers built jointly. They share `write_blob_meta`, whose signature changes, so neither call site compiles without the other, and the two regression tests cover them together.
  - Strict Lua fences: one piece, built jointly. The flipped tests and the fixtures pass only once the check exists and is wired into `parse.rs`, and the docs describe the check.
  - Leftover traces: one piece, built jointly. The edits are small and independent, and one set of `rg` checks accepts them all.

<step-1>

### Step 1: Publish the cache sidecar before the blob [completed]

- Component: Cache sidecar ordering
- Tests:
  - `cargo nextest run --locked -p gateway-local --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p gateway-local --all-targets --all-features`
  - `cargo fmt --all --check`
- Setup: read the root `AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`. If a build script fails for missing UI dependencies, run `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first.

Red first. Add the two tests below to `crates/gateway/local/src/cache/tests.rs` before any other edit. Run `cargo nextest run --locked -p gateway-local --all-features publishes_no_blob_whose_sidecar_write_failed`, and confirm each test fails on `assert!(!destination.exists())`. If one already passes, keep it, skip the matching writer change, and record that half of D1-1 as false in the commit message body.

- The fault, in both tests: after constructing the store or cache, call `fs::create_dir_all(meta_path(&destination))`, so `File::create` in `write_synced` fails on the directory.
- `artifact_store_publishes_no_blob_whose_sidecar_write_failed`:
  - Setup: `FakeServer::new(body)` and `ArtifactStore::new(temp.path())`. Get `destination` from `BlobCache::new(temp.path())?.destination(&url)`.
  - The first `ensure_model(&url, None)` returns an error. Then `assert!(!destination.exists())`, which fails at fdb20bad5.
  - Remove the directory at `meta_path(&destination)` and call `ensure_model(&url, None)` again. `assert_eq!(server.requests(), 2)`, which fails at fdb20bad5, where the count is 1.
  - `BlobCache::list()` equals exactly one `CacheEntry` whose `source` is the URL, `path` is `destination`, `sha256` is `hex_sha256(body)`, and `size_bytes` is `body.len()`. At fdb20bad5 the list is empty.
- `download_to_cache_publishes_no_blob_whose_sidecar_write_failed`:
  - Same fault. `download_to_cache(&url, None, ..)` returns an error, and `assert!(!destination.exists())` fails at fdb20bad5.
  - Remove the directory and retry. The returned `CachedBlob`'s sidecar at `meta_path(&blob.path)` parses as `BlobMeta`, and `server.requests() == 2`.

Work:

- `crates/gateway/local/src/cache.rs`:
  - `write_blob_meta` becomes the only code that builds and writes a `BlobMeta`. It takes the file to measure separately from the blob path it derives the sidecar path from, and it returns the `size_bytes` it recorded. It stays `pub(crate)`. Update its doc so it covers both writers instead of naming only `ArtifactStore`.
  - `BlobCache::download_to_cache`: after the digest check, run `remove_cache_entry(&self.root, &destination)`, then `write_blob_meta` measuring `staging`, then `rename_confined(&self.root, &staging, &destination)`. Delete the inline `BlobMeta` build, its `serde_json::to_vec`, and its `write_synced` call, and build the returned `CachedBlob` from the size `write_blob_meta` returns. Update the comment above the removal so it describes remove, then sidecar, then rename. Drop any import left unused.
- `crates/gateway/local/src/artifacts/blob.rs`, `ArtifactStore::ensure_blob_with_progress`: the destination is already absent at the download step, so only the sidecar write moves. After the digest check, call `crate::cache::write_blob_meta` measuring `staging`, with the digest `expected_digest.as_deref().unwrap_or(&actual)`. Then `rename_confined(&self.cache, &staging, destination)`. Then, for pinned assets only, the existing `.verified` marker `validate_cache_path` and `write_marker_best_effort`, as today. The pinned verified-hit call near line 51 keeps measuring `destination`.

Acceptance:

- Neither writer publishes a blob into `models/<key>/<filename>` before its complete `<file>.meta.json` sidecar is on disk, and removing whatever sat at the destination always comes before the sidecar write.
- A sidecar write failure returns the error with no blob published. The completed `.part` has no provenance marker, so the next attempt downloads from zero. A sidecar without a blob is skipped by `BlobCache::list`, is not a hit, and is overwritten by the next download.
- The sidecar JSON format is unchanged. There is no hash-on-hit repair and no repair of blobs already on disk.
- Unchanged and green without edits: `download_to_cache_downloads_verifies_and_writes_sidecar`, `download_to_cache_rejects_digest_mismatch_and_keeps_the_partial`, `download_to_cache_resumes_an_interrupted_partial`, `sidecar_less_blob_is_not_a_hit_and_is_replaced`, `corrupt_sidecar_is_skipped_and_not_a_hit`, `concurrent_publishers_converge_on_one_download`, `mismatched_pin_against_sidecar_forces_redownload`, `artifact_store_downloads_appear_in_the_cache_listing`, and every test in `crates/gateway/local/src/cache/tests-listing.rs`.

</step-1>

<step-2>

### Step 2: Make every non-conforming Lua fence a parse error [completed]

- Component: Strict Lua fences
- Tests:
  - `cargo nextest run --locked -p runvil-parser --all-features`
  - `cargo nextest run --locked -p runvil --all-features`
  - `cargo nextest run --locked -p runvil-engine --all-features` (includes `every_shipped_prompt_parses_offline` over the shipped `prompts/` tree)
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-parser -p runvil --all-targets --all-features`
  - `cargo fmt --all --check`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p runvil-parser --no-deps --all-features`
  - `rg -n -i "near-miss|stay inside prose|top-level" crates/runvil-internal/parser/src/fence.rs crates/runvil-internal/parser/src/parse.rs crates/runvil-internal/parser/src/error.rs` (must print nothing)
- Setup: read the root `AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`. New Markdown fixtures use LF line endings.

The rule, from the Functional Specification:

- A Lua fence is an opening line, then body lines, then a closing line. The opener and the closer are exactly the lines below. "Exactly" means nothing before or after on the line. The `\n` or `\r\n` terminator is not part of the line, so LF and CRLF files both work.
- The rule covers the whole body after the frontmatter (the preface before the H1, the H1, and every section) and runs before any Lua compiles.
- Exception: lines inside the body of another fenced code block are never inspected. That is how a document shows a Lua fence as an example.
- Everything else fails with `ParseErrorKind::Fence`, located at the offending line. The first violation in source order wins. Within one fence, the label rule is checked before the opener rule.
  - Label (unchanged): a fence whose label starts with `lua` in any letter case but is not exactly `lua`, such as `Lua`, `luau`, or `lua extra`.
  - Opener: a fence labeled exactly `lua` whose opening line is not the opener. Covered shapes: a trailing space or tab, one to three spaces of indent, a space between the backticks and the label, tildes, four or more backticks, and a fence inside a block quote or list item.
  - Closer: a fence that starts with the opener but whose closing line is not the closer. Covered shapes: a trailing space or tab, indent, and four or more backticks. An unclosed fence keeps its existing per-section "is not closed" error and its location wording.
  - Lookalike: a line outside every fenced code block whose text, after leading spaces and tabs, is three or more backticks or tildes, then optional spaces or tabs, then `lua` in any letter case. These are places where the Markdown parser reports no fence: an indented code block (four or more spaces) and a raw HTML block.
- The lines and messages, verbatim. The label message interpolates the offending label:

~~~text
the opener:         ```lua
the closer:         ```
label:              unrecognized fence label `lua extra`; the only Lua fence label is `lua`
opener, lookalike:  `lua` fence opening line must be exactly ```lua
closer:             `lua` fence closing line must be exactly ```
~~~

Red first. Make every test and fixture change below before touching `fence.rs` or `parse.rs`. Run `cargo nextest run --locked -p runvil-parser --all-features` and `cargo nextest run --locked -p runvil --all-features invalid_prompt_files_report_public_error_contracts`, and confirm each new or changed test fails on the assertion named with it. The exception guard is the only test that passes both before and after. If any other one passes at fdb20bad5, stop and report it instead of changing the test.

Parser tests, in `crates/runvil-internal/parser/src/tests/`:

- Shape test. Flip `h1_shape_near_misses_of_an_exact_lua_label_remain_prose` (`h1_fences.rs`) and `section_fence_markers_must_be_exact` (`section_fences.rs`) into error tests, and rename each to state the new behavior, for example `non_exact_lua_openers_fail_in_the_h1_and_the_preface` and `non_exact_lua_openers_fail_in_a_section`. Each shape is an opening line, `return 1`, and a closing line that matches it:
  - the opener followed by a trailing space, and followed by a trailing tab
  - three backticks, a space, then `lua`
  - the opener indented by one space, and by three spaces
  - `~~~lua`, closed by `~~~`
  - four backticks then `lua`, closed by four backticks
  - the opener inside a block quote, and inside a list item
  - Each must fail with kind `Fence` and the opener message, in the H1, in a section, and in the preface before the H1. Assert the message the way the neighboring label tests do (`assert_unrecognized_label` in `h1_fences.rs`, and equality on `error.to_string()` in `section_fence_label_near_misses_are_parse_errors`). At fdb20bad5 each one parses, so `expect_err` fails.
- Closer test. Update `non_exact_section_closing_before_another_lua_fence_is_a_parse_error` (`section_fences.rs`) to assert kind `Fence` and the closer message. Keep its three bad closers: the closer plus a trailing space, the closer indented by two spaces, and four backticks.
  - At fdb20bad5 the message is "section `S` `lua` fence is not closed exactly", so the message assertion fails.
  - Add a case with no later Lua fence, where the bad closer is followed by prose and then a closed `python` fence. At fdb20bad5 this fails with kind `Lua`, because the Lua source swallowed the prose, so the kind assertion fails.
- Lookalike test (new, `section_fences.rs`): a section holding the opener indented by four spaces after a blank line, and a section holding a `<div>` HTML block whose next line is the opener, with the Lua body, the closer, and `</div>` after it. Each must fail with kind `Fence` and the opener message. At fdb20bad5 both parse as prose.
- Ordering test (new, `h1_fences.rs`, beside `unrecognized_lua_label_fails_before_any_lua_compiles`, which it mirrors): an exact H1 Lua block followed by a section with an opener violation. `Prompt::parse` reports only `PARSE_STARTED` then `PARSE_FAILED`, with no Lua compilation events, and the error text does not contain the section's Lua source. At fdb20bad5 the parse succeeds.
- Exception guard (new, `section_fences.rs`, beside `section_markers_inside_longer_fences_remain_prose`): each of the following parses, with the inner text in the section's prose. This passes at fdb20bad5 by design: it guards the exception rather than proving a fix.
  - the user's example verbatim:

~~~markdown
````
some crap example
```lua caca
blahblah
```
more
````
~~~

  - a four-backtick fence holding the opener followed by a trailing space
  - a four-backtick fence holding the opener indented by four spaces

Facade fixtures, in `crates/runvil/tests/prompts/invalid/`, each modeled on `unrecognized-lua-label.md` (frontmatter with `name`, a `description` saying why the file is invalid, and `runvil: 0`, then an H1 holding the bad fence, then a valid `## Run` section):

- `lua-fence-opener.md` (`name: lua_fence_opener`): a Lua fence that opens with `~~~lua` and closes with `~~~`. Use tildes, because a trailing-space fixture would not survive editors that strip whitespace.
- `lua-fence-closer.md` (`name: lua_fence_closer`): a Lua fence that starts with the opener and closes with four backticks.
- Add both to `INVALID_FIXTURES` in `crates/runvil/tests/suite/parsing.rs` (the array length goes from 4 to 6), the opener entry first, with `ParseErrorKind::Fence` and the fragments `fence opening line must be exactly` and `fence closing line must be exactly`.
- At fdb20bad5 the opener fixture parses, so `invalid_prompt_files_report_public_error_contracts` reports "fixture invalid/lua-fence-opener.md unexpectedly parsed". The closer fixture fails there with the old "is not closed" message, so its fragment check fails too.

Work:

- `crates/runvil-internal/parser/src/fence.rs`:
  - Replace `first_unrecognized_lua_label` with one whole-body check, for example `pub(super) fn first_fence_violation(body: &str) -> Option<(Range<usize>, FenceViolation)>`, with `pub(super) enum FenceViolation { Label(String), Opener, Closer }`. The range covers the offending line without its terminator.
  - One pulldown-cmark pass over the body, built on `fence_openings` or a sibling of it, yields every fenced code block with its opening line, its label, and its extent from `into_offset_iter`. Check each block in this order: the label rule, then the opener rule, then the closer rule.
  - Take the closing line from the block's own extent in the event stream, not from a separate scan of the text after the opener, so the check and the Markdown parser always agree on where a fence ends. A block whose extent ends without a closing-fence line is unclosed. Report nothing for it and leave it to `split_section_blocks`.
  - The lookalike scan walks the body's lines and skips every line inside a reported fenced block's extent. That skip is the nested-fence exception, and it keeps opener lines from being checked twice. A remaining line that matches the lookalike pattern is an `Opener` violation.
  - Return the first violation in source order across both passes.
  - `split_section_blocks` keeps using exact openers and `extract_exact_fence`. Change its `opening < pos` branch, which returns "section `{section}` `lua` fence is not closed exactly", to the existing internal `ParseErrorKind::Structure` error "internal section fence classification mismatch". Once the whole-body check passes, that branch is unreachable, so no author-facing message may describe it. The unclosed-fence error from `extract_exact_fence` and its prologue, epilog, and middle location wording stay as they are.
- `crates/runvil-internal/parser/src/parse.rs`: at line 133, call the new check on `body`, map each variant to its message, and return `Error::parse_at(ParseErrorKind::Fence, line_range, message)`, as the label case does today. Update the `use crate::fence::...` import.
- Docs. State the strict rule and the nested-fence exception, using the rule names Label, Opener, Closer, and Lookalike rather than "near-miss" or "top-level":
  - `Prompt::parse`, the `Fence` bullet (`parse.rs` lines 40-44)
  - `ParseErrorKind::Fence` (`error.rs` lines 175-176)
  - the `fence.rs` module doc (lines 1-8)
  - `fence_openings`: it yields every fence the Markdown parser reports, including fences inside block quotes and list items, and never a fence inside another code block's body (D1-13)
  - `exact_lua_openings`: drop "top-level"
  - the new check's doc
  - `split_section_blocks`: drop "Near-miss fence forms stay inside prose" and the "top-level" wording
  - `extract_exact_fence`: drop "Near-miss closing markers are part of the source"
  - Then run `rg -n -i "near-miss|stay inside prose|top-level" crates/runvil-internal/parser/src` and fix any line that still describes the old fence rule. The hits that stay are the heading-hierarchy "top-level section" lines in `build.rs`, `detail.rs`, `prompt.rs`, and `tests/headings.rs`, and the test messages about label near-misses.

Acceptance:

- Unchanged: the label rule and its message, the set of Lua-like labels, frontmatter handling, and section splitting.
- Unchanged and green without edits: `h1_label_near_misses_fail_whatever_the_opener_shape` (the label rule still wins on four-backtick and indented openers), `label_near_misses_inside_longer_fences_remain_prose`, `section_markers_inside_longer_fences_remain_prose`, `fence_labels_not_starting_with_lua_stay_in_prose`, `non_lua_fence_stays_in_prose`, `unclosed_reserved_section_fences_are_location_errors`, the CRLF tests `section_compiles_prologue_and_epilog_around_prose` and `unrecognized_lua_label_in_the_h1_fails_the_parse`, every test in `line_mapping.rs`, and the Engine suite's `every_shipped_prompt_parses_offline`.
- No shipped prompt, fixture, or Workshop agent file needs an edit beyond the two new fixtures.
- Accepted gaps from the Decision Record's Risks: a Lua-looking line in an indented code block that is itself inside a block quote or list item is not caught, and a prose line that starts with an inline code span delimited by three backticks wrapping `lua x` now fails as a lookalike.

</step-2>

<step-3>

### Step 3: Remove the slot-era test traces and the stale tool-offering comments [completed]

- Component: Leftover traces
- Tests:
  - `cargo nextest run --locked -p gateway-local -p runvil-parser -p runvil-lua -p runvil-engine -p runvil-types -p runvil --all-features`
  - `rg -n -i "slot.era|tools\.(add|always|offered|add_local)\b" crates/runvil-internal/lua` (must print nothing)
  - `rg -n "offers every" crates/runvil-internal` (must print nothing)
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo check -p gateway --no-default-features`
  - `cargo fmt --all --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `cargo deny --log-level error check`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p runvil --no-deps`
- Setup: read the root `AGENTS.md`, `crates/runvil-internal/lua/AGENTS.md`, `crates/runvil-internal/engine/AGENTS.md`, and `crates/runvil-internal/types/AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`. The pre-push hook is a bash script. The docs-claims scan, the headless gateway check, the workspace clippy run, and `cargo deny` above are its commands in PowerShell form.

Work:

- `crates/runvil-internal/lua/src/tools/tests-offering.rs`:
  - Module doc, line 3: remove "and the removed slot-era functions", and fix the list's punctuation so the sentence still reads.
  - `the_slot_era_functions_are_not_installed` (line 229): drop the four `== nil` values (`tools.add`, `tools.always`, `tools.offered`, and `tools.add_local`) from the Lua chunk and from the result tuple, and keep the check that the six `tools` entries are functions. Rename it, for example `the_tools_namespace_installs_its_six_functions`. Keeping the positive half pins the installed `tools` surface.
- Rewrite four comments to say that every catalog tool is available to the prompt's Lua, which calls one by id, and that a section offers one to its model with `tools.offer`. Match the `RunContext::tools` doc in `crates/runvil-internal/engine/src/execute/config.rs`: "The catalog lists every tool the run can use: Lua calls one by id, and a section offers one to its model with `tools.offer`." Keep each comment's intra-doc links, such as [`ToolCatalog`] and [`ToolId`], intact.
  - `crates/runvil-internal/engine/src/tools.rs` line 3, the module doc
  - `crates/runvil-internal/types/src/tools/registry.rs` line 13, the `ToolCatalog` doc
  - `crates/runvil-internal/types/src/tools/descriptor.rs` line 17, the `ToolDescriptor` doc ("The Engine offers every descriptor to the prompt's Lua by id")
  - `crates/runvil-internal/engine/src/execute/tests/context.rs` line 147: say that the fixture Plugin's tools are in the run's catalog.

Acceptance:

- No removed slot-era tool names remain in the Lua tests, the renamed test passes, and no comment claims the Engine offers every catalog tool.
- This is the last work commit. Every exit check above passes on the final tree, and no structural ratchet is added.

</step-3>

</execution-plan>
