---
name: Host identifier rename
overview: "Rename every code name and message string that still uses \"host\" (or \"engine\"/\"harness\") in a sense other than the defined term, in seven planned commits (plus a line-ceiling split or a residual fix if a check calls for one) appended to branch terminology-engine-harness-host so PR #104 picks them up. Self-contained: a fresh agent can execute it from this file alone."
todos:
  - id: s1-setup
    content: "Step 1: read the Context section, confirm the branch state and the Linux compile baseline, overwrite vibe/2026-09-30-2-host-identifier-rename.md with this plan (scratch paths replaced by <scratch> and <sweep>) and run every later step from that copy, and create the scratch folder and rename.py"
    status: completed
  - id: s2-realbackend
    content: "Step 2: rename HostBackend to RealBackend and the real-filesystem names (vfs module and files, facade, public-api.txt bless, vfs.md, mount fixture names, the real-backend locals in suite/vfs.rs, tests, docs), with the plan record"
    status: completed
  - id: s3-cicerone
    content: "Step 3: run tools/cicerone.md in update mode for promptforge scoped to vfs.md with baseline 75d616f6; commit its page changes if any"
    status: completed
  - id: s4-lua
    content: "Step 4: rename the Lua Engine-globals names (inject_values, install_engine_globals, EngineGlobal, engine_globals module, engine_type helper, registry key), messages, Lua-sense test names, strings and fixture names, and the guide quote"
    status: completed
  - id: s5-test-harness
    content: "Step 5: rename RunHost, run_with_host, run_host, test_support/host.rs, the host locals across the engine crate (scripted, with a keep list), the Harness-sense test names and strings, and the Harness-sense facade doctest lines"
    status: completed
  - id: s6-modules
    content: "Step 6: rename performers-host.rs to performers-builtin.rs and execute/engine.rs to walk_target.rs"
    status: completed
  - id: s7-ui
    content: "Step 7: rename the DOM container names, LazyPanelHost, the harness() test helpers and config-ui harness.mjs in the TypeScript UIs, with exact wording for every check text"
    status: pending
  - id: s8-finish
    content: "Step 8: rename HOSTED_OFFER and SyntheticHost, qualify the database-engine and speech-engine names, add the retired seeds, update the AGENTS.md Using-the-terms bullets"
    status: pending
  - id: s9-final
    content: "Step 9: full gates plus the headless gateway check, identifier residual and sense checks, push, update the PR #104 description"
    status: pending
isProject: false
---

# Host identifier rename

## Context (read first; this plan assumes no other memory)

- Repository: `c:\Users\Vinnie\cursor\promptforge`. Branch `terminology-engine-harness-host`, tracking `origin/terminology-engine-harness-host`. `origin` is the fork `vinniefalco/promptforge`; `upstream` is `cppalliance/promptforge`. [PR #104](https://github.com/cppalliance/promptforge/pull/104) merges this branch into `upstream/master`. New commits go on this branch; `git push` updates the PR. No new PR.
- HEAD at planning time: `75d616f6`. The branch holds four commits: `78075746` (Definitions and prose sweep), `b3e194f8` (capitalization), `5854e29b` (rewords), `75d616f6` (rulebook guard). The only untracked file is the old draft `vibe/2026-09-30-2-host-identifier-rename.md`.
- The terms: read the `## Definitions` section of the root [AGENTS.md](promptforge/AGENTS.md). Engine = the `promptforge` crates (parses, steps, emits effects). Harness = the `harness` crates (steps the Engine, performs effects, keeps the run log; in Engine tests the code that steps the Engine plays the Harness's part). Host = the application (Workshop, Papergate). Prose already follows these terms; this plan changes code names and message strings.
- Rules that bind every edit: the root `AGENTS.md` (500-line file ceiling in crates with a `## Invariants` marker, flat source directories with kebab sibling files for one or two children, model-friendly error messages), and the workspace rules: plain English, no em dashes or double dashes, Engine/Harness/Host capitalized in prose. Commit messages follow `c:\Users\Vinnie\cursor\.cursor\rules\commit.mdc` (subject of 60 characters or fewer, one paragraph, optional bullets, never mention the plan).
- Environment (Windows, PowerShell):
  - Set `$env:PYTHONIOENCODING="utf-8"` before Python. Windows PowerShell 5.1 writes UTF-16 for `>` and plain `Out-File`, and UTF-8 with a byte-order mark for `Out-File -Encoding utf8`; a mark at the start of a commit message lands in its subject. Write commit-message files with the Write tool (or `[IO.File]::WriteAllText($path, $text, [Text.UTF8Encoding]::new($false))`); scratch lists and logs may use `Out-File -Encoding utf8`.
  - Use `rg` in the shell for exhaustive searches; the Grep tool truncates long lines.
  - Never put an edit and the check that observes it in one parallel batch; run dependent steps one after another.
  - Run cargo commands one at a time (they share the target-directory lock).
  - Spawn every subagent (Task tool) with `run_in_background: true`, including each one `tools/cicerone.md` dispatches in Step 3. Wait for a subagent's completion notification before any step that reads its result. A background subagent never edits the working tree while another step stages or commits.
  - git's "CRLF will be replaced by LF" warnings are benign (`eol=lf` in `.gitattributes`).
  - Do not add `2>&1` to cargo commands: Windows PowerShell 5.1 turns cargo's stderr text, including its standing "unused workspace dependency `fastrand`" warning, into NativeCommandError records and a nonzero exit code. Judge a cargo step by its own result lines and by `$LASTEXITCODE` of a plain run.
  - Never touch `c:\Users\Vinnie\cursor\promptforge2` or `promptforge3` (other checkouts). Never edit dated `vibe/` records except the plan record in Step 1.
- Scratch: put new scripts and logs in `<scratch>\`. The earlier sweep left `<sweep>\gates.ps1` (runs all 14 root `AGENTS.md` gates; `-Log <path>` names the log) and `pr-body.md` (the PR #104 description).

## Decisions (fixed; do not revisit)

- Keep, because they already mean the Host: `HostSnapshot`, `set_host`, the `host` field and `host()` getter on the Harness bindings, Workshop's `host_snapshot`, the Host-sense test names listed under Step 5, the `Origin::new("host")` label in the Plan-mode step at `crates/promptforge/src/vfs.md:158`, and the strings "User input is unavailable in this host; continue without it.", "..., and this host provides none", "...service this host does not provide...", "the host withdrew the wait", "host notes", "The host's current model", and the Engine test strings "the host's cancel ends the run" (`execute/tests/run_termination.rs:91`), "the host's started_at" (`execute/tests/run_inputs.rs:58`), "the host's flags are kept" (`run_inputs.rs:133`) and "nothing the host must supply" (`execute/tests/model_tasks.rs:57`).
- Exempt (never renamed): network names (`require_loopback_host`, `crates/shared-loopback/src/host.rs`, `max_per_host`, `url::Host`, `header::HOST`, `location.host`, `isLoopbackHost`, `canonical_host`, `host_is_loopback`, webfetch and DNS test names such as `allow_exact_admits_only_the_named_host`), cargo-dist names, and Cargo's host and target vocabulary in the build crates (`host_triple`, `host_os`, `host_arch`, `discover_host_target`). `SyntheticHost` is not exempt: its doc comment calls it "a synthetic Windows machine", so Step 8 renames it.
- `HostBackend` is public; it is renamed with no deprecated alias. Papergate updates its import.
- The retired names join the existing retired-symbol scan (approved).
- `inject_host` becomes `inject_values` rather than a longer name such as `inject_engine_globals`, to limit rewraps near the 500-line ceiling.
- `a_context_without_a_host_handle_...` is Harness sense: the engine's docs give the cancel handle to the Harness (`execute/config.rs:268`, `execute/run.rs:198`). A neutral `cancel_handle` name would be false, because `RunContext` has a `cancel_handle()`.
- An unqualified "engine" outside the Engine sense gets its qualified word: a database engine's error is "the database error", and outside `crates/gateway/stt/` the speech engine is "the speech engine" (Step 8).

## Method (every commit)

1. `git mv` the listed files and fix their `mod`, `use` and `#[path]` lines by hand. Apply the commit's maps with `rename.py` (below) over the listed globs.
2. Run `cargo fmt --all` (longer names rewrap) and `XTASK`. If a file is now over 500 lines, split it first, in its own commit: `git stash`, split that file at HEAD as a pure move (a kebab sibling `foo-bar.rs` wired with `#[path = "foo-bar.rs"] mod bar;`), run `XTASK` and the crate's tests, commit as "Split <file> under the line ceiling", `git stash drop`, and redo item 1.
3. Handle the commit's by-hand items (locals, test names, strings) from the lists below, run `cargo fmt --all` again, then the commit's checks. If a by-hand edit pushes a file over 500 lines, split it inside the commit and name the split in the message.
4. Confirm no old name remains: `rg -n -w "<old names joined by |>" crates guide tools README.md -g "!**/target/**" -g "!**/node_modules/**"` prints nothing. The root `AGENTS.md` names `HostBackend`, `RunHost` and `inject_host` until Step 8 rewrites that bullet, so only Step 8 adds it to the search. The plan record in `vibe/` is outside these paths.
5. `git add -A`, write the message per the commit rule, `git commit -F <file>`. Nothing else edits the working tree while a commit is staged.

`rename.py` (save to the scratch folder; run from the repo root as `python <scratch>\rename.py <map.json> <glob>...`). A map is a JSON object of old name to new name (an old name can be a phrase of several words, as in Step 4); an optional `"__keep__"` list names substrings, and a line that contains one is left as it is. It reads the map as `utf-8-sig`, so a map written by `Out-File` with a byte-order mark still loads.

```python
"""Applies whole-word renames from a JSON map to files matched by globs, in one pass, longest name first.
A "__keep__" list in the map names substrings; a line that contains one is left as it is."""
import json, pathlib, re, sys
mapping = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8-sig"))
keep = mapping.pop("__keep__", [])
skip = {"target", "node_modules", "dist", "vibe", ".git"}
word = re.compile("|".join(rf"\b{re.escape(old)}\b" for old in sorted(mapping, key=len, reverse=True)))
for pattern in sys.argv[2:]:
    for path in pathlib.Path(".").glob(pattern):
        if not path.is_file() or skip & set(path.parts):
            continue
        text = path.read_text(encoding="utf-8")
        new = "".join(
            line if any(k in line for k in keep) else word.sub(lambda m: mapping[m.group(0)], line)
            for line in text.splitlines(keepends=True)
        )
        if new != text:
            path.write_text(new, encoding="utf-8", newline="")
            print(path)
```

Standard checks, used below:
- `CLIPPY-WS`: `cargo clippy --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`.
- `CLIPPY`: `CLIPPY-WS`, then `cargo clippy --locked -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`. Steps 2 and 7 run both (Step 2 changes the facade surface; Step 7 changes the UI sources `workshop-server` bundles). Steps 4, 5, 6 and 8 change no facade surface and none of those three crates, so they run `CLIPPY-WS` only; the final gates run both.
- `XTASK`: `cargo test --locked -p build-xtask` (ceiling, tiers, retired seeds).
- `DOC-ENGINE`: `$env:RUSTDOCFLAGS="-D warnings"; cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items; $env:RUSTDOCFLAGS=""` (CI's docs job runs it; `gates.ps1` does not).
- `LINUX`: `cargo clippy --locked -p promptforge-vfs --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings`. It compiles the `#[cfg(unix)]` code that a Windows build skips; the Linux target is installed and the crate has no dependencies, so it needs no C toolchain.

## Step 1: setup

- `git status --short --branch` shows the branch, in sync with origin, with only the draft untracked. If HEAD is not `75d616f6`, read `git log 75d616f6..HEAD` and recheck the inventories below before editing.
- Copy this plan file over `vibe/2026-09-30-2-host-identifier-rename.md` (it joins Step 2) before any other edit, with a small script in the scratch folder. In the copy, replace the full path of the scratch folder named `host-identifier-rename` with `<scratch>` and the full path of the one named `terminology-sweep` with `<sweep>`, so no staging path stays in a committed file. Run every later step from that copy, not from this file. When a step's checks pass, set its todo to `completed` in the copy before the step's commit, so the commit holds the record. If a step makes no commit (Step 3 when the survey prints `NOTHING`), the status change rides in the next commit. Set Step 9's todo before the push; the PR edit that follows changes no file. In the copy, `<scratch>` and `<sweep>` mean those two folders.
- Create the scratch folder and `rename.py`.
- Run `LINUX` once on the untouched HEAD. It must pass, so a later failure points at an edit. If it fails for a toolchain reason, note that and rely on Linux CI for the `#[cfg(unix)]` code.

## Step 2: Rename HostBackend to RealBackend

Files (`git mv`): `crates/promptforge-internal/vfs/src/host.rs` to `real.rs`; directory `host/` (`files.rs`, `resolve.rs`, `tests.rs`, `tests/atomicity.rs`, `tests/links.rs`, `tests/semantics.rs`) to `real/`. In `vfs/src/lib.rs:28` `mod host;` becomes `mod real;` and `:40` `pub use host::HostBackend` becomes `pub use real::RealBackend`. The facade re-exports it at `crates/promptforge/src/lib.rs:149` (`pub use promptforge_vfs::HostBackend;`); the map renames that line, and `cargo fmt --all` moves it to its alphabetical place in the `vfs` module (after `Policy`, before `Stat`).

Map: `HostBackend` to `RealBackend`, `HostAccess` to `RealAccess`, `HostRoot` to `RealRoot`, `identity_to_host` to `identity_to_real`, `host_from` to `real_from`, `host_to` to `real_to`. Globs: `crates/**/*.rs`, `crates/promptforge/src/*.md`, `crates/promptforge-internal/vfs/README.md`, `tools/cicerone/plans/promptforge.md`. Not `AGENTS.md` (Step 8 rewrites that bullet). At `75d616f6`, `host_from`, `host_to`, `HostRoot` and `HostAccess` occur only in the vfs files, so the workspace-wide glob is safe.

By hand:
- The local `host` (an OS path) in `real.rs` (35), `real/files.rs` (2), `real/resolve.rs` (7) and `real/tests/links.rs` (3 of its 4; the fourth is the string at `:90`, below) becomes `real`, with comments that name it; the local `host_file` in `real/tests.rs:147-153` becomes `real_file`. (The `host` hits in `real/tests.rs` and `real/tests/semantics.rs` are the strings below.)
- The message `"the host backend is read-only..."` (`real.rs:209`) becomes `"the real backend is read-only..."`; the lint reason at `vfs/src/lib.rs:145` "...is a host-OS notion" becomes "...is an operating-system notion".
- vfs test names and strings: `an_identity_backend_maps_virtual_paths_directly_to_host_paths` becomes `..._to_real_paths`; `backslashes_from_windows_hosts_are_separators` (`path.rs:198`) becomes `..._windows_machines_...`; `"promptforge-vfs-host-test-{}-{}"` becomes `"promptforge-vfs-real-test-{}-{}"`; `"reading the host file"` becomes `"reading the real file"`; `"host semantics test"` (7) becomes `"real semantics test"`; `links.rs:90` "the host refused" becomes "the operating system refused".
- `vfs/src/detail-tests.rs`: the arbitrary mount at `:277-293` (`"/host"`, `"host/secret.txt"`, `"/host/secret.txt"`, and the comment at `:289`) becomes `"/mount"`, `"mount/secret.txt"`, `"/mount/secret.txt"`. At `:465-467` the local `host` is the Harness's store view (its comment says so): it becomes `harness`, and the content it writes, `b"host"`, becomes `b"harness"`.
- Facade [crates/promptforge/src/vfs.md](promptforge/crates/promptforge/src/vfs.md): the `## HostBackend` heading (line 294) and lines 18, 25, 64, 79, 296-300, including doctest code; the doctest comments at `:21` "Make a host folder" and `:51` "writes the host file" say "real folder" and "real file". Leave `:158` `Origin::new("host")` (Host sense). Then `cargo +nightly-2026-09-05 xtask api --bless` rewrites `crates/promptforge/public-api.txt`: the listing is alphabetical, so the 7 `HostBackend` lines are removed and 7 `RealBackend` lines are added at their sorted places (`git diff --numstat` prints `7 7`).
- Engine `execute/tests/suite/vfs.rs`: `"host-backend"` to `"real-backend"`, `"vfs-invariance-host"` to `"vfs-invariance-real"`, expect and assert texts "the host backend" and "host-backed" to "the real backend" and "real-backed"; "the host holds the run's store views" to "the Harness holds the run's store views". Test name `fanout_interleaving_is_invariant_across_memory_and_host_backends` becomes `..._memory_and_real_backends`. The real-backend side of that test (`:267-301`) renames its locals: `host_vfs` to `real_vfs`, the local `host` at `:274-293` to `real`, and `host_result` to `real_result`. The `host` locals at `:118-156` hold the test Harness; Step 5's script renames them, so this must land first.
- Engine `execute/tests/suite/exec_flow/store_failures.rs`: the test `a_store_at_the_root_cannot_reach_a_host_mount_beneath_it` mounts a `MemoryBackend` at `"/host"`, so it uses neutral mount words. `.mount("/host", ...)` (`:165`) and `.write("/host/secret.txt", ...)` (`:171`) become `"/mount"` and `"/mount/secret.txt"`; the Lua path `'host/secret.txt'` (`:157`, no leading slash) becomes `'mount/secret.txt'`; `Origin::new("host seeding")` (`:169`) becomes `"mount seeding"`; `'the host path is not in the store'` (`:158`) becomes `'the mounted path is not in the store'`; `'the host file is simply absent: '` (`:159`) becomes `'the mounted file is simply absent: '`; `.expect("the host mount seeds")` (`:172`) becomes `"the mount seeds"`; the comments at `:148-150` and `:167-168` that say "real directory" and "real mount" become "mounted directory" and "the mount"; the test name becomes `a_store_at_the_root_cannot_reach_a_mount_beneath_it`. A different test in the same file asserts at `:320` `"the captured write is performed by the host: {records:?}"`, which becomes `"the captured write is performed by the Harness: {records:?}"`.
- `crates/promptforge/tests/suite/prepare.rs`: `HostBackend` (via map, `:19` and `:76`); `.expect("the temp dir roots the host backend")` (`:76`) becomes `"the temp dir roots the real backend"`; `.expect("run a writes the host file")` (`:90`) becomes `"run a writes the real file"`; the test name at `:69` becomes `two_runs_writing_the_same_real_file_through_the_shared_base_conflict`. Leave `"The host's current model"` (`:172`) and the Host comments at `:71` and `:167` (Host sense).
- Docs: `vfs/README.md:3`, `tools/cicerone/plans/promptforge.md:447` (via map).

Checks: `CLIPPY`; `LINUX`; `cargo nextest run --locked -p promptforge-vfs -p promptforge-engine -p promptforge --all-features`; `cargo test --locked --doc -p promptforge -p promptforge-vfs --all-features`; `$env:RUSTDOCFLAGS="-D warnings"; cargo doc --locked -p promptforge --no-deps; $env:RUSTDOCFLAGS=""`; `cargo +nightly-2026-09-05 xtask api --check`; `XTASK` (watch `vfs/src/detail-tests.rs`, 471 lines). `LINUX` compiles the `#[cfg(unix)]` code (`real/resolve.rs`, `real/files.rs`, the `real/tests` files) that Windows nextest skips.

Commit (with the plan record): "Rename HostBackend to RealBackend". The body states that `promptforge::vfs::HostBackend` is now `promptforge::vfs::RealBackend` with no alias.

## Step 3: Refresh the vfs facade page

The root `AGENTS.md` requires [tools/cicerone.md](promptforge/tools/cicerone.md) in update mode when a facade item is renamed. Follow that file as written (its Sub-Agent Dispatch section says how to brief each task; spawn every task with `run_in_background: true`), with `CRATE=promptforge`, `MODE=update`, `PAGES=vfs.md`, and `BASELINE=` the parent of Step 2 (`75d616f6` unless Step 1 found new commits). The baseline is required: without it the survey uses the last commit that touched `vfs.md`, which is Step 2, and finds nothing. Run `python tools/cicerone/scripts/survey.py promptforge update --pages vfs.md --baseline <sha>`, then its status loop. It leaves changes uncommitted, keeps intermediates in `target/cicerone-promptforge/`, and its report and findings go to the cabinet as **output**. Review the `vfs.md` diff; rerun the Step 2 doc and doctest checks; commit as "Refresh the vfs facade page for RealBackend". If the survey prints `NOTHING`, there is no commit.

Expect a full one-page run, not a heading touch-up: `HostBackend` and `RealBackend` differ in their last path segment, so the survey sees one removed and one added item and prints `SCOPE`, not `NOTHING`. The tool dispatches roughly 15 to 25 subagents and takes tens of minutes to hours, and it stops on a dirty facade page, so Step 2 must be committed first. Do not start the next `status.py` until every subagent of the current batch has finished, never overlap a cargo step with another cargo command, and compare the tool's `vfs.md` with the Step 2 page before committing, because it can overwrite hand edits.

## Step 4: Rename the Lua Engine-globals names

Files (`git mv`): `crates/promptforge-internal/lua/src/host.rs` to `engine_globals.rs`, `host-store.rs` to `engine_globals-store.rs`. In `lua/src/lib.rs` `mod host;` (line 125) becomes `mod engine_globals;` and the `use host::` paths at lines 126, 127 and 156 become `use engine_globals::`; `lua/src/error.rs:212` `crate::host::store_error_message` becomes `crate::engine_globals::store_error_message`; in `engine_globals.rs:3` `#[path = "host-store.rs"]` becomes `#[path = "engine_globals-store.rs"]`.

Map: `inject_host_with_var` to `inject_values_with_var`, `inject_host` to `inject_values`, `install_host_apis` to `install_engine_globals`, `host_injected` to `values_injected`, `HostGlobal` to `EngineGlobal`, `host_type` to `engine_type`. Globs: `crates/promptforge-internal/**/*.rs`, `crates/promptforge-internal/lua/src/*.lua`, `crates/harness-internal/**/*.rs`, `crates/promptforge-internal/lua/AGENTS.md`. (References: 29 `.rs` files in the lua and engine crates; `host_type` in `__impl_coro.lua` lines 42, 275, 408, `__impl_tasks.lua` 12, 21, 32, 53, 61, 80, 84, 223, 227, 241, 258, 272, `__impl_fanout.lua` 12.)

Messages (product text; update producers and every assertion together):
- `lua/src/globals.rs:142` label `"a host global"` becomes `"an Engine global"`. Its asserts follow: `parser/src/contract/tests.rs:142-143` (`("argv", "a host global")` and `("store", "a host global")`) both become `"an Engine global"`, and `lua/src/prelude-tests.rs:216` `message.contains("host global")` becomes `message.contains("Engine global")`, because the new label no longer contains "host global".
- `lua/src/prelude.rs:200` `"which is already a host global"` becomes `"which is already an Engine global"`; asserts `"which is reserved as a host global"` in `lua/src/prelude-tests.rs:230` and `engine/src/execute/tests/preludes.rs:179` become `"...an Engine global"`.
- `"section VM host values have not been injected"` (`vm/state.rs:45`, `vm/run.rs:82`, `vm/install.rs:191`, `prose.rs:100`), `"...were already injected"` (`install.rs:132`), `"...were not injected"` (`install.rs:349`) say "Engine values"; the asserts check only the tails and stay. The quotes in `engine/src/error.rs:198` and `lua/src/error.rs:50` follow.
- Registry key `host-store.rs:28` `"promptforge.host.store_phase"` becomes `"promptforge.engine.store_phase"`.
- Guide: `guide/src/language/02-file-structure.md:340` has two occurrences of "a host global" on that one line, the category list (`a host global`) and the example message ("is reserved (a host global): ..."). Both become "an Engine global" (replace every occurrence on the line, not the first); it is the only guide quote of a changed name or message. Regenerate the exports with `cargo run --locked -q -p build-user-guide`. The tool has no check mode and rewrites all three exports every time, so afterwards `git diff --stat -- guide` must list only `02-file-structure.md` and `promptforge-language-guide.md`. Leave `guide/src/language/06-arguments.md:61`: its "later host value" is an example of an argument the Host supplies.

Test names (Lua sense):
- `sealed_host_values_keep_their_metatable_protection` to `sealed_engine_values_...`
- `host_validation_still_rejects_a_bad_builder_record` to `engine_validation_...`
- `a_global_that_collides_with_a_host_global_fails_naming_both_sides` to `..._an_engine_global_...`
- `compatibility_chunk_logs_interleave_with_host_operations` to `..._engine_operations`
- `shared_functions_resolve_host_globals_when_called_from_a_later_chunk` and engine `shared_function_resolves_host_globals_when_called` to `engine_globals`
- `section_vm_requires_delayed_single_host_injection` to `..._single_value_injection`; `section_vm_host_injection_bypasses_shared_global_metatables` to `section_vm_value_injection_...`
- engine `shared_library_calls_host_apis_at_load_time` to `..._engine_globals_...`; `a_non_function_compactor_is_the_calls_error_in_the_hosts_type_names` to `..._the_engines_type_names`; `a_one_byte_limit_fails_host_injection_with_teardown_observations` to `..._value_injection_...`; `a_prelude_defining_ui_or_item_collides_on_a_host_that_binds_neither` to `..._on_a_run_that_binds_neither`; `a_host_callback_failure_caught_by_pcall_is_the_same_error_table` to `an_engine_callback_failure_...`
- harness-runner `a_normal_alias_for_the_ask_tool_runs_beside_the_untouched_host_globals` to `..._engine_globals`

Phrase map (scripted; run it after the identifier map and before the by-hand items below). About 85 `expect(...)` lines in the Lua crate alone hold the phrases in the next paragraph, so do not edit them one at a time. Save this as a second map and run `rename.py` with it over `crates/promptforge-internal/**/*.rs` and `crates/harness-internal/**/*.rs`. The script replaces longer keys first, so "host values must inject" is never cut short by "host must inject":

```json
{
  "section VM host values": "section VM Engine values",
  "host values have not been injected": "Engine values have not been injected",
  "host values must inject": "Engine values must inject",
  "host values inject": "Engine values inject",
  "host must inject": "values must inject",
  "host injects": "values inject",
  "host APIs must install": "Engine globals must install",
  "the host APIs install": "the Engine globals install",
  "host injection cannot fail": "value injection cannot fail",
  "host injection installs the messages namespace": "value injection installs the messages namespace",
  "host injection installs the models table": "value injection installs the models table",
  "host injection failure": "value injection failure",
  "programs cannot run before host injection": "programs cannot run before value injection",
  "host values cannot be replaced": "Engine values cannot be replaced",
  "the shared function must mutate host state when called": "the shared function must mutate Engine state when called",
  "read as the host set them": "read as the Engine set them"
}
```

The map covers the message producers, the quotes in `engine/src/error.rs:198` and `lua/src/error.rs:50`, and the `expect` and assert texts. The by-hand items below cover everything the map does not (labels, test names, fixture names, Lua text, doc comments). The phrase check at the end of this commit proves nothing was missed.

Test strings: the `expect(...)` texts "host must inject", "host injects", "host values inject", "host values must inject", "host APIs must install", "the host APIs install", "host injection cannot fail", "host injection installs the messages namespace", "host injection installs the models table", "the shared function must mutate host state when called" become "values must inject", "values inject", "Engine values inject", "Engine values must inject", "Engine globals must install", "the Engine globals install", "value injection cannot fail", "value injection installs...", and "...mutate Engine state..."; engine `observations.rs:352` "host injection failure..." becomes "value injection failure..."; engine `suite/global_metatable.rs:147` Lua text 'both still read as the host set them' becomes '...as the Engine set them'. Also:
- `lua/src/tests/section_vm.rs:107` "programs cannot run before host injection" becomes "programs cannot run before value injection"; `:114` "host values cannot be replaced" becomes "Engine values cannot be replaced".
- `lua/src/globals-tests.rs:95` "argv and prose read as the host set them after {after}" becomes "...as the Engine set them after {after}".
- `lua/src/prelude-tests.rs:217` "...and the host global: {message}" becomes "...and the Engine global: {message}".
- Engine `execute/tests/live_infer.rs`: `:104` "shared function must resolve host globals when called" becomes "...resolve Engine globals when called"; `:138` "top-level shared host calls must succeed" becomes "top-level shared Engine calls must succeed"; the prompt names `shared-host` (`:93`) and `shared-host-load` (`:115`) become `shared-globals` and `shared-globals-load`, and their headings `# Shared Host` (`:94`) and `# Shared Host Load` (`:116`) become `# Shared Globals` and `# Shared Globals Load`; "later host value" (`:102`, `:107`, the input of the Lua-sense test at `:92`) becomes "later canned value".
- `engine/src/lua/tests/globals.rs:95` "...which RESERVED_NAMES does not list as a host or Lua global; list it there" becomes "...as an Engine or Lua global; list it there". Step 5's locals script would otherwise turn it into "a harness or Lua global".
- `crates/harness-internal/runner/tests/it/prepare-input.rs:405` "the alias global is the ask tool, and input, store, and tools are the host's" becomes "...are the Engine's".
- Comments around renamed calls that say "host injection" or "host APIs" follow.

Phrase check, after the by-hand edits (identifier searches cannot see message text): `rg -n -e "host global" -e "host or Lua global" -e "section VM host values" -e "host injection" -e "host APIs" -e "host must inject" -e "host injects" -e "host values" -e "host calls" -e "mutate host state" -e "host set them" -e "are the host's" -e "promptforge.host.store_phase" crates/promptforge-internal crates/harness-internal guide/src` prints nothing, and so does `rg -n -w host crates/promptforge-internal/lua crates/promptforge-internal/parser`, because neither crate has a Host-sense `host`.

Checks: `CLIPPY-WS`; `cargo nextest run --locked -p promptforge-lua -p promptforge-engine -p promptforge-parser -p promptforge -p harness-runner --all-features`; `cargo test --locked --doc -p promptforge-lua -p promptforge-engine --all-features`; `DOC-ENGINE`; `XTASK` (watch `lua/src/tests/tool_scoping.rs` 482 lines, `engine/src/execute/tests/live_infer.rs` 481, the engine's `observations.rs` 478, and `lua/src/globals-tests.rs` 459).

Commit: "Rename the Lua Engine-globals names".

## Step 5: Rename the Engine test-support Harness names

Files (`git mv`): `crates/promptforge-internal/engine/src/test_support/host.rs` to `harness.rs`; in `test_support.rs:42` `pub(crate) mod host;` becomes `pub(crate) mod harness;` and `:50` `pub use host::{..., RunHost}` follows.

Map: `RunHost` to `RunHarness`, `run_with_host` to `run_with_harness`, `run_host` to `run_harness`, `delta_host` to `delta_harness`. Globs: `crates/promptforge-internal/engine/**/*.rs`, `crates/promptforge/tests/**/*.rs`.

Locals (scripted, second map): after Steps 2 and 4, every bare `host` and `_host` left in the engine crate holds the test Harness (a `RunHost`, also reached through helpers such as `prepare_run`, `scheduler_context` and `TokioDriver::new`), apart from a few Host-sense strings. At `75d616f6` that is about 660 `host` and 12 `_host` tokens, 529 of them in spots like `(ctx, host)`. A renamed local raises no compile error, so do not hunt them by hand. After the first map, run `rename.py` with this map over `crates/promptforge-internal/engine/**/*.rs`:

```json
{"host": "harness", "_host": "_harness", "__keep__": ["this host provides none", "The host's current model", "the host's cancel", "the host's started_at", "the host's flags", "nothing the host must supply"]}
```

The script also renames `fn host(self) -> RunHost` in `execute/tests/suite/support.rs:53` and its two callers, and the backticked `host` in doc comments (`test_support.rs:170,175,197`, `test_support/tokio_driver.rs:170`, `execute/tests/context-tools.rs:57,77,92`). Snake_case names such as `a_host_seeds_...` are not whole-word matches, so it leaves test names alone. Then review the string lines it changed (`git diff -U0 | rg '^\+.*\x22[^\x22]*\bharness\b'`): prose inside a string says "Harness", and the strings listed below take their listed wording. Afterwards `rg -n -w "host|_host" crates/promptforge-internal/engine` prints only lines that hold a keep string. ("this host provides none" matches four lines: `requirements.rs:122` and `requirements-tests.rs:94,109,134`, all Host sense.)

By hand:
- Test names (Harness sense): `a_chat_round_streams_its_deltas_to_the_host`, `a_nested_infer_round_streams_no_deltas_to_the_host`, `the_hosts_client_serves_a_run_the_context_never_names` (`the_harness_client_...`), `the_run_forwards_every_buffered_event_to_the_host_observer_in_order`, `a_bound_alias_with_no_implementation_in_the_host_table_resumes_as_a_tool_error`, `tasks_concurrency_clamps_to_the_host_ceiling`, `tasks_concurrency_clamps_to_the_host_ceiling_and_reads_the_effective_limit_back`, `a_compactors_own_string_raise_reaches_the_host_with_the_reason_tag`, `a_context_without_a_host_handle_shares_its_one_flag_with_prepare_and_the_run`, `a_run_ends_its_scope_at_done_while_the_host_still_holds_its_store_views`, `dropping_a_run_before_done_ends_its_scope_while_the_host_still_holds_its_store_views`, `a_captured_store_function_called_after_load_reaches_the_host_as_a_store_effect`, and facade `prepare_fills_a_slot_by_id_against_a_host_supplied_catalog` (`harness_supplied`): "host" becomes "harness".
- Keep (Host sense): `a_host_seeds_and_extracts_through_the_prepared_handle_with_no_real_files` (the test plays a Papergate-style Host).
- Strings (the script has already turned each "host" here into a lowercase "harness"; find them by line and set the listed wording):
  - `test_support/tokio_driver-performers.rs:57` "...no implementation in the host's table" becomes "...in the Harness's table", with the assert in `execute/tests/tool_call_arm.rs:211`; `tool_call_arm.rs:212` "the raised message names the host table, got: {out}" becomes "...names the Harness's table, got: {out}".
  - `execute/tests/live_infer.rs:33,53` "host answer" becomes "canned answer"; the prompt name `host-client` (`:36`) becomes `harness-client` (the script does that) and its heading `# Host Client` (`:37`, capitalized, so the script skips it) becomes `# Harness Client`; `:50` "the host's client must serve the run" and `:57` "...gone to the host's client" say "the Harness's client".
  - `execute/tests/effects.rs:291` "...reach the host's hook live, in order" becomes "...reach the Harness's hook live, in order".
  - `run/tests.rs:414` "...so the host can drop what it holds" becomes "...the Harness can..."; `context-tests.rs:100` "...past the host's parse events" becomes "...the Harness's parse events"; `happens_before-concurrency.rs:75` Lua text 'clamped to the host ceiling of 4, got ' becomes 'clamped to the Harness ceiling of 4, got '.
  - The lint reason at `execute/run.rs:112` "the host API takes the context by value: ..." becomes "the public API takes the context by value: ...".
- Facade doctest code in the Harness sense (outside Step 3's `vfs.md` scope): `crates/promptforge/src/transport.md:96` "The host loop's chat arm" becomes "The Harness loop's chat arm"; `effect.md:149` "your host answers with" becomes "your Harness answers with" (a column header over a wide rule, so the longer word keeps the alignment); `ids.md:54,219` "the run waits on an effect this host holds" becomes "...this Harness holds".
- Benches: `engine/benches/models_loop.rs` (via both maps).

Checks: `CLIPPY-WS` (covers benches); `cargo nextest run --locked -p promptforge-engine -p promptforge --all-features`; `cargo test --locked --doc -p promptforge --all-features`; `$env:RUSTDOCFLAGS="-D warnings"; cargo doc --locked -p promptforge --no-deps; $env:RUSTDOCFLAGS=""`; `DOC-ENGINE`; `XTASK` (watch `execute/tests/tool_loop.rs` at 496 lines, `model_tasks.rs` 481, `live_infer.rs` 481, `happens_before.rs` 478, `scheduler/failures.rs` 474). A trial `rustfmt` run over the renamed text kept `tool_loop.rs` at 496 and `live_infer.rs` at 483, so no split is expected; the real `cargo fmt` is the test.

Commit: "Rename the Engine test-support Harness names".

## Step 6: Rename the internal modules named host and engine

- `crates/harness-internal/runner/src/performers-host.rs` to `performers-builtin.rs` (the performers the runner supplies itself: timer, store, task-events read). In `performers.rs:38-39` `#[path = "performers-host.rs"] mod host;` becomes `#[path = "performers-builtin.rs"] mod builtin;` and `:43` `pub use host::{LogTaskEvents, TokioTimer, VfsStore};` becomes `pub use builtin::...`.
- `crates/promptforge-internal/engine/src/execute/engine.rs` to `walk_target.rs` (walk-target resolution). In `execute.rs:42` `mod engine;` becomes `mod walk_target;`; module paths `engine::` become `walk_target::` in `execute/section_context-construct.rs` (1), `execute/scheduler/h1.rs` (1), `execute/scheduler/walk.rs` (1), `execute/tests/exec_flow.rs` (2). The module list in the `execute.rs` header is alphabetical: its bullet at `:15`, `` `engine` - the walk-target resolution helpers. ``, becomes `` `walk_target` - the walk-target resolution helpers. `` and moves after the `tools` bullet (`:37`). `cargo fmt --all` moves the `mod walk_target;` line to its sorted place the same way.

Checks: `CLIPPY-WS`; `cargo nextest run --locked -p harness-runner -p promptforge-engine --all-features`; `DOC-ENGINE`; `XTASK`. Residual: `rg -n -e "mod engine;" -e "execute::engine" -e "super::super::engine::" crates/promptforge-internal/engine` prints nothing, and so does `rg -n "^//! - .engine. " crates/promptforge-internal/engine/src/execute.rs`. Leave `crates/gateway/stt/engine` and build-xtask's `mod engine_guards` alone.

Commit: "Rename the internal modules named host and engine".

## Step 7: Rename the DOM container names in the TypeScript UIs

Options-API renames (update every caller in the same commit):
- `LazyPanelHost` (`crates/workshop/platform/panel-registry.ts:78`, local `host` at `:89-90`) to `LazyPanelContainer` and `container`.
- `ModalOptions.host` to `container` in `crates/workshop/look/modal.ts:47,81,228` and `crates/shared-ui/modal.ts:49,83,230`; `PanelDialogOptions.host` to `container` in `crates/workshop/ui/src/parts/shared/panel-dialog.ts:34`. Callers: `editor-panel.ts:292,417,487`, `run-panel.ts:224,268`, `parts/workspace/add-folder.ts:41`, `look/test/shared-modal.mjs:63,97,135,154`, `crates/gateway/config-ui/ui/src/components/confirm-modal.ts:34`.

Parameter and local renames: `addFolderToWorkspace(host)` (`add-folder.ts:33`), `openReviewDiff(host)` (`review-diff.ts:21,94`), `createApplyOverlay(host)` (`apply-overlay.ts:56,153`), `confirmDialog(host)` (`confirm-modal.ts:24`) take `container`, and the JSDoc that names `host` follows (`add-folder.ts:26`, `review-diff.ts:20`, `confirm-modal.ts:21`, `apply-overlay.ts:54`); the local in `crates/workshop/platform/text-control-service.ts:65-69` becomes `editable`; test locals `host` to `container` in `look/test/shared-modal.mjs` (all 25 uses), `look/test/icons.mjs:49-57`, `ui/test/markdown-render.mjs:63-65`, `ui/test/text-control-service.mjs:131,134,157,213`; `editHost` to `editRoot` (`text-control-service.mjs:271-279`); `emptyHost` to `emptyContainer` (`command-center.mjs:166-174`). Leave the other `host` uses in these trees: the bind host in `settings-page.ts`, `location.host`, the loopback-host comments in `panel-bridge.ts`, and the guard in `docs-claims.mjs`.

Strings: `"host-toolbar"` (`ui/test/chat-box.mjs:822`) to `"owner-toolbar"`; `"reboot-the-host"` (`ui/test/gateway-config-bridge.mjs:168`, an unknown action) to `"reboot-the-machine"`. Check texts, with their exact new wording ("the owning part" is the part that embeds the chat box, as in the `AGENTS.md` chip entry):
- `look/test/shared-modal.mjs:78` "the overlay mounts into the host" becomes "the overlay mounts into the container".
- `ui/test/status-indicators.mjs:69` "the host registers no indicator of its own (got ...)" and `:70` "the host exposes no recording port" say "the status bar" (the subject is `bar`, a `StatusBar`).
- `ui/test/chat-box.mjs:807` "...so the host can name the blocker" becomes "...so the owning part can name the blocker"; `:834` "...after the host's own" becomes "...after the owning part's own"; `:843` "...and leaves the host's" becomes "...and leaves the owning part's".
- `ui/test/stt-stream.mjs:947` "...names the other window, not the host blocker" becomes "...not the owning part's blocker".
- `ui/test/chatbox-boundary.mjs:38` "the host's access-control vocabulary" becomes "the owning part's access-control vocabulary"; `:50` "...reaches into the host layers" becomes "...reaches into the embedding layers".
- `ui/test/gateway-config-bridge.mjs:193` "the panel hosts an iframe immediately (no async origin probe)" becomes "the panel embeds an iframe immediately (no async origin probe)".
- `crates/gateway/config-ui/ui/src/pages/settings-sections.test.mjs:189` "the inert hosting bind stays out of the editor: the gateway hosts no workshop listener" becomes "the inert listener bind stays out of the editor: the gateway runs no workshop listener"; `:219` "a fresh section omits the inert hosting bind" becomes "a fresh section omits the inert listener bind".

Test scaffolding named harness: the helper `harness()` in `crates/workshop/ui/test/agent-stt.mjs:387` (26 calls) and `agent-session-view.mjs:139` (9 calls) becomes `setup()`, and the comment at `agent-session-view.mjs:46`, "...mounts in every harness.", becomes "...mounts in every setup."; `crates/gateway/config-ui/ui/src/harness.mjs` becomes `test-support.mjs` (`git mv`), and its 23 importers (`src/**/*.test.mjs`) follow (`rg -l "harness.mjs" crates/gateway/config-ui/ui/src`).

Checks, in CI's order: in `crates/workshop`: `npm run typecheck --workspaces --if-present`, `npm run build --workspace ui`, `npm test --workspaces --if-present`; in `crates/gateway/config-ui/ui`: `npm run typecheck`, `npm run build`, `npm test` (the config UI tests load `dist/app.js`, so without the build they can pass on a stale bundle; `dist/` is gitignored); then `CLIPPY` (both runs bundle the UIs: `workshop-server` bundles the Workshop UI, `gateway-config-ui` bundles `shared-ui`). Residual: `rg -n -w -e LazyPanelHost -e editHost -e emptyHost -e host-toolbar -e reboot-the-host crates -g "!**/node_modules/**" -g "!**/dist/**"` and `rg -n -e "harness\.mjs" -e "\bharness\(" crates/workshop crates/gateway/config-ui -g "*.mjs" -g "*.ts" -g "!**/node_modules/**" -g "!**/dist/**" -g "!**/docs-claims.mjs"` print nothing. The last glob is required: `crates/workshop/ui/test/docs-claims.mjs:109` holds the rulebook guard's own regex source `harness(?![-_/:\w])`, which matches `\bharness\(` and is not a helper to rename. Leave that file alone.

Commit: "Rename DOM container and test-helper names in the UIs".

## Step 8: Finish the rename

- `HOSTED_OFFER` (`crates/gateway/cloud-providers/src/providers/foundry.rs:61,100,368`) to `SERVED_OFFER`; the assert message at `:369`, "the hosted-offer filter drops the mirrored registry", to "the served-offer filter...". The value `"standard-paygo"` and the `AzureOffers` field are Azure's names and stay.
- `SyntheticHost` (`crates/build-llama-cuda/src/bundle.rs:523`, inside `#[cfg(test)] mod tests`) becomes `SyntheticMachine`, and its locals `let host = SyntheticHost::new()` become `machine`: run `rename.py` with `{"SyntheticHost": "SyntheticMachine", "host": "machine"}` over `crates/build-llama-cuda/src/bundle.rs` (every bare `host` in that file, `:633-786`, is one of these locals). The test `non_windows_host_is_rejected` (`:632`) becomes `non_windows_machine_is_rejected` by hand, because its name is not a whole-word match.
- Database-engine and speech-engine names:
  - `crates/harness-internal/log/src/error.rs`: the local `engine` (`:126-128`, `:158-160`, a `turso::Error`) becomes `database`; "its engine cause" (`:130`) and "the engine cause" (`:134`) say "database cause"; the test names become `the_database_variant_reaches_the_database_error_through_the_log_wrapper` (`:125`) and `the_database_wrapper_hands_back_the_database_error_it_wraps` (`:157`).
  - `crates/workshop/workspace/src/workspace_file/tests.rs`: the test name `the_database_variant_reaches_the_engine_error_through_the_shared_wrapper` (`:15`) becomes `the_database_variant_reaches_the_database_error_through_the_shared_wrapper`; "its engine cause" (`:20`), "the engine cause" (`:23`), "an engine failure" (`:79`) and "the engine failure" (`:298`) say "its database cause", "the database cause", "a database failure" and "the database failure".
  - `crates/gateway/app/tests/it/realtime_stt.rs:251` "native engine loads" becomes "the native speech engine loads" (leave `:183`, `base64::engine::`).
- Retired seeds: in [crates/build-xtask/src/engine_guards.rs](promptforge/crates/build-xtask/src/engine_guards.rs) `RETIRED_SEEDS: [&str; 8]` becomes `[&str; 17]` with `"HostBackend"`, `"HostAccess"`, `"HostRoot"`, `"identity_to_host"`, `"inject_host"`, `"inject_host_with_var"`, `"install_host_apis"`, `"host_injected"`, `"HostGlobal"` added; its doc comment says the list holds the sans-I/O plan's retired names and the names the terminology rename replaced. The existing seed test iterates the array, so no test edit is needed.
- Root `AGENTS.md`, Using the terms (inside `## Definitions`, which the rulebook guard skips): in the "Names defined outside this repository" bullet, "Cargo's host triple" becomes "Cargo's host and target vocabulary"; in the next bullet, drop the sentence "Identifiers that use "host" in another sense, such as `HostBackend`, `RunHost`, and `inject_host`, keep their old names until a rename lands." and end the bullet with "Code names follow the same terms."

Checks: `XTASK`; `CLIPPY-WS`; `cargo nextest run --locked -p gateway-cloud-providers -p build-llama-cuda -p harness-log -p workshop-workspace --all-features`; `npm test` in `crates/workshop` (rulebook guard). Method item 4 for this commit adds the root `AGENTS.md` to its search and covers every old name from Steps 2 to 8.

Commit: "Retire the old host names and finish the rename".

## Step 9: Final checks and push

1. Full gates: `powershell -NoProfile -ExecutionPolicy Bypass -File <sweep>\gates.ps1 -Log <scratch>\gates.log` (about 15 minutes; all 14 lines PASS), then the headless gate that `gates.ps1` lacks: `cargo check --locked -p gateway --no-default-features`.
2. Identifier residual: `rg -n -o -i "[A-Za-z_]*host[A-Za-z_]*" -g "*.{rs,ts,mjs,js,lua}" -g "!**/target/**" -g "!**/node_modules/**" crates | Out-File -Encoding utf8 <scratch>\ids.txt`, then group the tokens. Every token must be in the keep list, the exempt list, a network word (`localhost`, `hostname`, `host_str`, URL and DNS code in webfetch, web-search, models config, gateway, shared-loopback), Cargo vocabulary in the build crates, the capitalized word Host in comments (`Host`, `Hosts`, `Host's`), or substring noise (`ghost`, `hostile`, `Hostx64`, `sechost`, `nHost`, `nhost`, `bhost`, `nghost`).
3. Sense check: the token check cannot see a wrong-sense `host`, because `host` is also a kept token, and it skips the facade doc pages. `rg -n -w "host|_host" crates/promptforge-internal/vfs crates/promptforge-internal/lua crates/promptforge-internal/engine crates/promptforge crates/harness/src` prints only lines that hold a keep string from Decisions: `Origin::new("host")` in `vfs.md`, "this host provides none", "The host's current model", "the host's cancel", "the host's started_at", "the host's flags", "nothing the host must supply".
4. Fix anything items 2 and 3 find in one more commit, "Rename the remaining host names", with the Method's checks for the crates it touches.
5. `git push`.
6. Edit `<sweep>\pr-body.md`: replace the `## Not in this PR` section with `## Code renames`, which lists the rename commits (and any split commits), says `promptforge::vfs::HostBackend` is now `RealBackend` (Papergate must update its import), names the extended retired-symbol scan, and keeps the line that the Host-sense messages stay. Then `gh pr edit 104 -R cppalliance/promptforge --body-file <that file>`.

## Review (efficiency, clarity, data flow)

- Order is required: Steps 2, 4 and 5 touch the same Engine test files (`suite/vfs.rs`, `store_failures.rs`, `live_infer.rs`), so they run one after another. Step 6 (Rust) and Step 7 (TypeScript) touch disjoint files but share one working tree: `git add -A` in Step 6 would sweep a concurrent worker's TypeScript edits into the wrong commit, and Step 6's clippy run bundles the gateway config UI sources (which Step 7 edits) mid-edit. They run one after another too.
- Step 5's locals script assumes every bare `host` left in the engine crate is Harness or Host sense, so Step 2's real-backend locals in `suite/vfs.rs` and Step 4's Lua-sense strings and fixture names must land first.
- Each commit runs only the crates it touches, and Steps 4, 5, 6 and 8 skip the Workshop clippy run; the full gate list runs once at the end (about 15 minutes) instead of seven times.
- Every step has what it needs: the maps and lists come from read-only inventories at `75d616f6`; Step 1 rechecks them if HEAD moved. Step 3's baseline is Step 2's parent (`75d616f6`). Step 9 needs the pushed branch and the scratch `pr-body.md`.
- Parallel work: none between commits (one working tree, `git add -A`, the cargo lock, Cicerone's doctest runs). Inside a step, read-only searches and Cicerone's own subagents run in the background.
- Ambiguities settled in this plan: every new name; the sense of each test name (Host-sense names listed as keep); the exact wording of every changed check text; the Host-sense strings the Step 5 script keeps; the mount fixtures in `store_failures.rs` and `detail-tests.rs` (a `MemoryBackend`, so neutral "mount" words); the build crates (Cargo vocabulary exempt, `SyntheticHost` renamed).
- Risks: rustfmt rewrap near the 500-line ceiling (Method item 2 splits first, in its own commit); the Step 5 script's string changes (reviewed by diff, with the keep list protecting Host-sense lines); Step 3's tool may rewrite more of `vfs.md` than the rename needs (reviewed as its own commit); Papergate breaks on `HostBackend` until it updates (stated in the PR).

