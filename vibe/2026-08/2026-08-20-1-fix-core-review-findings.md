---
name: Fix promptforge-core review findings
overview: "Fix all 140 findings from the promptforge-core code review: 4 must-fixes as individual commits, 13 should-fixes in 7 grouped commits, 123 nice-to-haves in 5 themed sweep commits. Each commit gated on tests + clippy."
todos:
  - id: preflight
    content: "Pre-flight: baseline cargo test + clippy green"
    status: completed
  - id: m1
    content: "M1: fence.rs closing-grammar panic fix + tests + commit"
    status: completed
  - id: m2
    content: "M2: structural parse-error classification + tests + commit"
    status: completed
  - id: m3
    content: "M3: add_local duplicate-alias rejection + tests + guide + commit"
    status: completed
  - id: m4
    content: "M4: README example rewrite + commit"
    status: completed
  - id: s1
    content: "S1: config.rs MissingEnv->Config + commit"
    status: completed
  - id: s2
    content: "S2: FileStore backslash confinement + commit"
    status: completed
  - id: s3
    content: "S3: exhaustive dialect/model error classification + commit"
    status: completed
  - id: s4
    content: "S4: three doc claims corrected + commit"
    status: completed
  - id: s5
    content: "S5: model_and_reply test defects + commit"
    status: completed
  - id: s6
    content: "S6: detached test servers via ScriptedGateway + commit"
    status: completed
  - id: s7
    content: "S7: live.rs record_error extraction + commit"
    status: completed
  - id: n1
    content: "N1: doc-accuracy sweep + commit"
    status: completed
  - id: n2
    content: "N2: production refactors + commit"
    status: completed
  - id: n3
    content: "N3: behavioral nits with tests + commit"
    status: completed
  - id: n4
    content: "N4: test assertion audit + commit"
    status: completed
  - id: n5
    content: "N5: test boilerplate sweep + commit"
    status: completed
isProject: false
---

# Fix promptforge-core review findings

Source of truth for every finding: `cabinet/_output/code-review-promptforge-core.md` (machine-readable: `cabinet/_scratch/code-review-promptforge-core/final_findings.json`). All commits land on `master` in `c:\Users\Vinnie\src\cursor\promptforge` (clean tree, tracks origin/master; do NOT push). Commit messages: lowercase imperative per repo history. Per AGENTS.md: prefer existing facilities, doc comments on public items, update docs when behavior changes.

## Pre-flight

- Baseline gate: `cargo test -p promptforge-core` and `cargo clippy -p promptforge-core --all-targets` must be green before any fix. If the baseline is red, stop and report.

## Phase 1 - must-fix, one commit each

### M1. `parser/fence.rs` - fence grammar mismatch panic

Defect: `exact_lua_openings` finds openings via pulldown-cmark's event stream, but `extract_exact_fence` ([fence.rs:146](promptforge/crates/promptforge-core/src/parser/fence.rs)) scans for an exact ` ``` ` closing line. pulldown accepts closings the exact grammar rejects (1-3 space indent, trailing spaces, extra backticks), so `pos` can advance past the next opening and `content[pos..opening]` panics at line 311.

Fix (preserves the locked exact-fence grammar):
- `exact_fence_openings` returns `(line_start, fence_end)` pairs by pairing pulldown Start/End events, so each opening knows where pulldown thinks its fence ends.
- `extract_exact_fence` gains a `limit: usize` parameter: scan lines only while `offset < limit`; if no exact closing line appears before the limit, return the existing `{label} fence is not closed` error. A non-exact closing thus fails loudly as "not closed" instead of panicking.
- Apply at both call sites (`split_h1`, `split_section_blocks`).
- Regression tests (parser/tests.rs): trailing-space closing, 2-space-indented closing, and extra-backtick closing each followed by a second exact ` ```lua ` fence produce a parse error, never a panic.

Commit: `fix the fence-grammar mismatch panic in split_section_blocks`

### M2. `parser.rs` - substring error classification

Defect: `classify_parse_error` ([parser.rs:69](promptforge/crates/promptforge-core/src/parser.rs)) substring-matches `Error::Parse` messages that interpolate user-controlled text; a list section named `frontmatter` or `fence` with an empty bullet misclassifies as `ParseErrorKind::Frontmatter/Fence`.

Fix: migrate every remaining string-based `Error::Parse` site to `Error::ParseStructured { kind, span: None }` with the correct kind - list.rs (3 sites -> List), fence.rs:157 (-> Fence), build.rs:231/249 (-> Frontmatter), parser.rs:394/402/409/414 and fence.rs internal-mismatch sites and build.rs:399/410 (-> Structure), execute.rs:219 (-> Structure). Then delete the substring fallback; any residual `Error::Parse` maps to `Structure`.

Regression tests: list section named `frontmatter` with an empty bullet yields `ParseErrorKind::List`; same for a section named `fence`.

Commit: `classify parse errors structurally instead of by message substring`

### M3. `lua/tools_bridge.rs` - add_local alias shadowing

Defect: the `add_local` closure ([tools_bridge.rs:222-243](promptforge/crates/promptforge-core/src/lua/tools_bridge.rs)) never checks for duplicates: a local alias colliding with a `tools.need`/`tools.always` alias (or a prior `add_local`) advertises two same-named tools and silently routes the alias to the local handler.

Fix: in the closure, after `validate_alias`, reject when `bindings.binding(&alias).is_some()` (declared alias) or `local.contains(&alias)` (already registered local). Error text in the existing family: `tools.add_local alias "x" duplicates a declared tool alias` / `is already registered as a local tool`. Fails at the assigning Lua line, consistent with the crate's fail-loud duplicate-alias discipline.

Regression tests (execute/tests/local_tools.rs): shadowing a declared alias errors loudly; re-registering the same local alias errors. Update the user guide's `tools.add_local` section to state the rejection.

Commit: `reject duplicate and declared aliases in tools.add_local`

### M4. `README.md` - non-compiling front-door example

Defect: [README.md:16-22](promptforge/crates/promptforge-core/README.md) calls `Prompt::parse_file` and `prompt.run()`, neither of which exists.

Fix: rewrite the example to the verified real API, mirroring the `no_run` doctest in [lib.rs:44-69](promptforge/crates/promptforge-core/src/lib.rs): `Prompt::parse(source, "readme", &NullObserver::default())` then the free `run(&prompt, "", ResolutionContext::new(&picker, &models), &[], &StoreRef::memory(), RunConfig::new("readme")).await`.

Commit: `fix the README usage example to the real API`

## Phase 2 - should-fix, grouped commits

- **S1. Error-variant routing:** `client/config.rs:126` - route scheme/host/credential/query rejections through `Error::Config` instead of `Error::MissingEnv` (a present-but-invalid URL currently reports "missing environment variable"). Test: invalid URL kinds as Config. Commit: `report invalid gateway URLs as config errors`.
- **S2. Windows confinement:** `store/file.rs:54` - `FileStore::resolve` splits only on `/`; also reject/split on `\` so a direct FileStore caller cannot escape root with `..\escape.txt` on Windows. Test: backslash traversal rejected. Commit: `close the FileStore backslash confinement gap on Windows`.
- **S3. Exhaustive error classification (cross-file G3):** `dialects/error.rs:69` catch-all -> explicit arms (`DialectNone => NoMatch`, `DialectTie => Tie`, rest => `Unknown`); `model/error.rs` `CompletionError::kind` wildcard-to-Config -> exhaustive match. Follow `execute/error.rs`'s pattern: a new `Error` variant fails to compile until classified. Commit: `make dialect and model error classification exhaustive`.
- **S4. Doc claims contradicted by code:** `dialects/error.rs:32-35` (reword the false "not constructible outside the crate" claim), `execute.rs:150` (drop "or omitted it" from the `RunErrorKind::Version` bullet - the omitted-key path kinds as Parse), `tools/output.rs:141` (`with_source` doc states the fixed `kind=Backend`). Commit: `correct three doc claims contradicted by reachable behavior`.
- **S5. model_and_reply test defects:** delete or differentiate the byte-identical twins (`model_and_reply.rs:459-468` vs `511-520`); fix `whitespace_only_prose_skips_model_without_binding` (line 401) so the whitespace prose actually reaches the skip path (move the prologue's early return into an epilog). Commit: `fix two model_and_reply tests that pin nothing`.
- **S6. Detached test servers:** replace hand-rolled axum servers whose spawned task `.unwrap()`s the serve result - `live_infer.rs:6-26` and `tool_scoping.rs:142-160` - with `ScriptedGateway::start(...)` + `gatewayed(addr)` (the EXEC-TESTS-003 hazard the owned harness exists to prevent). Commit: `route hand-rolled test servers through ScriptedGateway`.
- **S7. live.rs error recording:** extract `BindingState::record_error` to replace the four hand-copied lock-then-record blocks in `install_live_tools` ([live.rs:113-177](promptforge/crates/promptforge-core/src/lua/live.rs)); a future error path that forgets a step would silently drop the typed resolver error. Commit: `extract BindingState::record_error in install_live_tools`.

## Phase 3 - nice-to-haves, themed sweeps (all 123)

- **N1. Doc-accuracy sweep** (~25 findings): `# Errors` additions (transport `from_env`, `run_prose_inference`, `str_replace`, `build_sections`, `SharedTools::new`), stale references (`ModelCatalog`'s deleted `filter` method, twice), missing doc comments on uniformly documented files (handles.rs, live.rs, scope.rs, tools_bridge.rs, hardening.rs `scalar_return`, options.rs), wording fixes (dispatch.rs `results[i].0` tuple notation, `map_runtime_error` variant names, `status()` doc, dangling section comment in exit_rules.rs, user-guide line 77 fall-off wording, two test-comment corrections). Gate adds `cargo doc -p promptforge-core` (workspace denies broken intra-doc links).
- **N2. Production refactors** (~20 findings, no behavior change): helper extractions - dispatch.rs messages-array validation, codec.rs scanner skeleton, content.rs peel blocks, guide.rs signature assembly, sys.rs `enrich_sys_field`, host.rs read-bounds + store-table installer, store.rs range-read prologue, subst.rs scalar-render arms, resolver.rs group-to-ids, vm.rs table-packing loops, program.rs compiler-VM builder, scope.rs alias-lookup blocks, build_sections block-building loop, `run_fanout_arms` setup/join blocks, `complete()` request-body builder, error.rs `BoxedSource` alias, gemma3 mod.rs parse_turn fixture helper.
- **N3. Behavioral nits** (~10 findings, each with a regression test): block_walk.rs LiveH1 model-check ordering moved below the empty-prose skip to match the Section arm; h1.rs `inject_host` failure routed through `h1_try!` so the teardown observation pair fires; `run_live_h1_block` drains the resolver callback-error recorder on success (no pcall-swallowed misattribution); `LocalTools::schemas`/`contains` map poison to `Error::Lua` like their siblings; `infer_direct` skips the spurious `MODEL_TURN_FAILED` on cancellation and its violation message names the actual entry point; decode.rs entry-point labels parameterized (no more "models.need" for a `models.default` failure); vm.rs jump-slot poison wording unified; `guarded_var` stores a rebuilt table so nested writes cannot bypass the assigning-line deep check; `compile_glob` gains a debug_assert on its validated precondition; `newlines_before` returns a structured error instead of panicking on a bad offset; `render_tool_guide` pins parameter key order; `default_log_byte_budget` renamed.
- **N4. Test assertion audit** (~15 findings): strengthen assertions to match test names (or rename): `prepare_request_is_identity` whole-body assert, `tools_mode_serde_round_trip` adds `from_str`, `logs_are_correlated_and_ordered_across_chunks` asserts ordered triples, `binding_rejects_unknown_and_duplicate_always_aliases` per-case messages, `captured_bindings_do_not_execute_h1_source` sets the flag or drops the vacuous assertion, `near_duplicates_are_forwarded` asserts pair identities, `undeclared_models_use_fails_loudly` checks the message, `infer_options` table case, `validate_alias` 65-char boundary, `invalid_middle_lua_fence_fails_parse` asserts `ParseErrorKind::Lua`, glob parity test made non-reflexive, `glob_snapshots_and_matches_outside_the_lock` name narrowed, `backend_ctor_classifies_and_hides_source` Display-opacity assert, `platform_unsafe_paths` exact 1024-byte boundary pin, `tool_error_classifies_and_hides_source` Display assert, `forwards_validated_optional_fields` asserts `include_domains` arrives, exec_flow jump test rename.
- **N5. Test boilerplate sweep** (~40 findings, includes the rest of cross-file G1): adopt `gatewayed(addr)`/`ScriptedGateway` everywhere (~30 byte-identical RunOptions/GatewayClient blocks across debug_and_counts, live_infer, local_tools, model_and_reply, tool_scoping, tool_loop, and mod.rs itself); extract the missing helpers: `add_local_md` fixture builder, gemma3 parse_turn body helper, normalize.rs envelope helper, `lua_error_message`, `numbered_fixture` adoption, store rejection-loop and mirrored range-pair helpers, compile/resolve prologue helpers (always.rs, integration.rs), fanout `shared_chunk` + `EventRecorder` assert helpers, parser/tests `src()` prefix helper and mlua line-mapping helper, axum bind/spawn helpers (model/transport, client/tests), `inspect_id()` helper, fixture-tool declarative builder in tests/mod.rs.

## Execution notes

- Must-fixes and should-fixes: direct edits in the main session (precision work, exact locations known from the review).
- N1-N5 sweeps: delegate to subagents, one per theme, each carrying the theme's finding list from the report; each subagent compiles, tests, lints, and commits its theme.
- Per-commit gate: `cargo test -p promptforge-core` + `cargo clippy -p promptforge-core --all-targets` green; doc commits also `cargo doc -p promptforge-core`. No commit unless green. No push.
- If a finding turns out to be wrong against the current code, skip it and note it - do not force it.