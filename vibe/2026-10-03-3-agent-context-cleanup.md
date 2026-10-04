---
name: Agent context cleanup
overview: Hide stale docs from the agent, one audited item at a time.
todos:
  - id: "1"
    content: Remove all rustdoc markdown includes (promptforge, harness, engine) and archive them, plus tools/cicerone, in promptforge-design/saved-docs/
    status: pending
  - id: "2"
    content: Move or delete every remaining doctest, remove the doctest steps, and add a tidy check that rejects new doctests
    status: pending
  - id: "3"
    content: Rewrite every README.md as a broad, stale-proof statement of responsibility
    status: pending
  - id: "4"
    content: Replace the docs landing page with one table that xtask site generates
    status: pending
  - id: "5"
    content: Remove all crates.io metadata from every Cargo.toml
    status: pending
  - id: "6"
    content: Move the three guides' chapters to promptforge-docs; promptforge CI clones it and builds the whole site
    status: pending
  - id: "7"
    content: Remove stale enforcement code and config
    status: pending
  - id: "8"
    content: One strict workspace lint table; no copies
    status: pending
  - id: "9"
    content: Root clippy.toml bans on installing process-global state
    status: pending
  - id: "10"
    content: Ban serde_json preserve_order in deny.toml
    status: pending
  - id: "11"
    content: Gate settings - docs-link-free clippy, CARGO_BUILD_WARNINGS, hakari verify, git hooks on
    status: pending
  - id: "12"
    content: Shrink AGENTS.md - delete what checks now enforce, fix the wrong fact, reword CSS
    status: pending
  - id: "13"
    content: Make the 500-line limit absolute, compiler-enforced, and encapsulation-safe
    status: pending
isProject: false
---

# Agent context cleanup

<product-contract>

## Product Requirements

PromptForge is at version 0 and changes fast: one vibe-coding session can land a hundred commits, after which hand-written docs are out of date, and agents that read them write code against wrong information. This plan tunes the `promptforge` repository for fast, accurate agent-driven development, while keeping what a human needs to verify the agents' work. It removes prose that drifts from code, moves the long-form guides out of the agents' working tree, replaces `AGENTS.md` rules with structural enforcement, and makes the 500-line file limit absolute and compiler-enforced without loosening encapsulation.

- Problem and users:
  - The users are the coding agents that edit the repository and the repository owner, who reviews their output.
  - Long-form docs inside the repository drift after every session, and agents treat the stale text as truth. These are the tutorial pages pulled into rustdoc with `include_str!`, the three mdBook guides under `guide/src/`, and the generated single-file guide exports under `guide/`.
  - Outside readers are not a goal at version 0. In the user's words: "optimize for maximizing vibe code output ... the human has to verify. This means the API has to be kept tight, the rustdoc attached to each symbol must be accurate, and cargo doc should produce HTML that is legible to the reviewer."
- Goals:
  - Optimize for build speed and correctness.
  - Remove things that can go stale with fast-changing code.
  - Prefer structural checks over `AGENTS.md` prose, and shrink `AGENTS.md` rather than expand it.
  - Inside `promptforge`, the only documentation is doc comments on declarations plus deliberately generic READMEs.
- Non-goals:
  - Polished docs for outside readers.
  - Publishing to crates.io, now or later.
  - Preserving the guides' git history.
  - Custom compiler lints (Dylint), ast-grep, Cursor hooks, a unified `cargo xtask check` command, and generators beyond the ones this plan names.
- Success criteria:
  - No markdown file is included as rustdoc anywhere in the workspace, no doctest remains or runs, and a tidy check rejects any new one.
  - The `tools/cicerone` doc generator, which wrote the deleted rustdoc pages, is archived out of promptforge.
  - Every `README.md` states only broad responsibility.
  - The docs landing page is a generated table with one row per published rustdoc site and per guide.
  - No crates.io metadata remains except `description` on the two shipped binaries.
  - The three guides' chapters live in `cppalliance/promptforge-docs`, and promptforge's CI still publishes the whole site at the same URL.
  - Every crate inherits one strict workspace lint table, the enforcement this plan names exists, and the `AGENTS.md` text it replaces is gone.
  - No Rust file in any crate exceeds 500 lines, a violation fails the build with split guidance, and no `pub(crate)` or `pub(super)` item is wider than its uses need.
- Constraints:
  - Changes land in the `promptforge` repository (`c:\Users\Vinnie\cursor\promptforge`), with two named exceptions. Archived rustdoc pages go to the `promptforge-design` repository (`c:\Users\Vinnie\cursor\promptforge-design`) under `saved-docs/<crate>/`, and `tools/cicerone` goes under `saved-docs/cicerone/`. Guide chapters go to the `promptforge-docs` repository (`c:\Users\Vinnie\cursor\promptforge-docs`). Touch nothing else in the surrounding workspace folder `c:\Users\Vinnie\cursor`.
  - Retired files are removed with git, and the history keeps them. In the user's words: "override the workspace rule. we have git."
  - Installers must not break: the Tauri desktop build of `crates/workshop/desktop`, and the cargo-dist release of `crates/gateway/app` configured in `dist-workspace.toml`.
  - The development machine runs Windows with PowerShell. The toolchain is the unpinned `stable` channel (`rust-toolchain.toml`), currently Cargo 1.99.
  - Prose written by this plan uses no em dashes and no double dashes.
- Open questions: None

## Functional Specification

The repository changes in three places: what an agent reads, what the build rejects, and how the docs site is assembled. An agent sees doc comments and generic READMEs, never tutorial or guide prose. A Rust file over 500 lines fails the build with instructions for a split that keeps encapsulation intact. The published site keeps its URL, and promptforge's CI rebuilds it from promptforge's rustdoc plus the guides checked out from `promptforge-docs`.

- Actors and workflows:
  - A coding agent edits code. The build, clippy, cargo-deny, and `cargo test -p build-xtask` report every rule this plan enforces, and each failure message states the fix.
  - The reviewer reads diffs of `crates/promptforge/public-api.txt` and the per-symbol rustdoc.
  - A docs author edits chapters in `promptforge-docs`. The site publishes them by the next nightly build.
- Inputs and outputs:
  - Site build inputs: the promptforge checkout, plus a `promptforge-docs` checkout whose path is in the environment variable `PROMPTFORGE_DOCS`.
  - Site build output: `target/site/`, holding the generated landing page, the three books, the four rustdoc sites, and the single-file guide exports as downloads.
- States and validation:
  - The landing page holds a title and one table, with no images and no other prose. Its columns are Documentation, Kind, and Covers. Its rows are four API references (`promptforge`, `harness`, `harness-gateway-client`, `harness-web`) and three guides (Prompt Language, Gateway, Workshop). Each Covers cell is a few broad words.
- Errors and recovery:
  - A `.rs` file over 500 physical lines fails its crate's build with the ceiling failure message from Technical Design. The recovery is a split under the split rule in Technical Design, which never widens visibility.
  - A compiled code block in a doc comment fails `cargo test -p build-xtask`, and the message says to move the example into a test.
- Security and privacy behavior:
  - Only `crates/gateway/app`, `crates/gateway/stt/whisper-ffi`, `crates/gateway-api-discovery`, and `crates/workshop/desktop` may contain unsafe code, each through a module-level `#[expect(unsafe_code, reason = ...)]`. Every unsafe block has its SAFETY comment on the line before it.
  - Library code never exits the process or installs process-global state. Only binary entry points do, under `#[expect]` with a reason.
- Acceptance criteria:
  - A fresh clone builds, tests, lints, and documents with the Project Survey commands.
  - A 501-line `.rs` file added to any crate fails that crate's `cargo build` with the message above.
  - `cargo test -p build-xtask` fails on each of these: `unsafe_code` expected outside the four crates; a crate without `[lints] workspace = true`; a crate whose `build.rs` doesn't call `build_ceiling::check()`; a `workspace-hack` file over 500 lines; a compiled code block or a `doc = include_str!` in a doc comment.
  - Turning on serde_json's `preserve_order` anywhere in the dependency graph fails `cargo deny check`.
  - The site workflow publishes rustdoc and books, and a change pushed only to `promptforge-docs` is published by the next nightly run.

</product-contract>
<implementation-contract>

## Technical Design

Enforcement moves out of prose and into the compiler, clippy, cargo-deny, and the repository's own tidy checks in `crates/build-xtask`. A new build-dependency crate gives every crate a compile-time file-size check, one workspace lint table governs every crate, and the docs site is built from promptforge plus a separate checkout of the guides. Rustdoc comes only from doc comments. Every step follows the rules in the first bullet.

- Rules every step follows:
  - Find each edit by its quoted text. Line numbers and counts in this plan are as of 2026-10-03, and they drift.
  - Remove retired files with `git rm`, because the history keeps them.
  - Commit edits to `promptforge-design` (`c:\Users\Vinnie\cursor\promptforge-design`) and `promptforge-docs` (`c:\Users\Vinnie\cursor\promptforge-docs`) in those repositories first, as separate commits, and push each one right away. The run does every push itself, and it never asks the operator to push.
  - Sub-agents never stage or commit in any repository. They delete retired files from the worktree outright, never moving them to `cabinet/_trash/`, because the user overrode that workspace rule for this plan ("we have git"). The step's promptforge commit records each removal, which is what `git rm` means in this plan. They write sibling-repository copies unstaged. The session running the step commits and pushes `promptforge-design` or `promptforge-docs` after the coding sub-agent returns and before the step's promptforge commit, and again after any later round that changes them.
  - The last step pushes promptforge after full verification passes. It then starts the site workflow with `gh workflow run site.yml`, and watches that run and the CI run for the pushed commit with `gh run watch` until both succeed. A failure in either is fixed in that same step.
  - A step that spans many crates divides the work among parallel sub-agents, one crate group each, with disjoint files. Every sub-agent leaves its changes unstaged, and the step's coding agent merges them, runs the step's checks, and returns as usual.
  - Parallel sub-agents share one worktree and one Cargo target directory. Each compiles only its own packages, and when a build fails only in files another sub-agent owns, it waits and retries instead of editing them. A pass that breaks a crate on purpose while it compiles, such as Step 9's narrow-then-restore loop, walks crates in dependency order in one sub-agent or scripted walker, or runs each sub-agent in its own git worktree whose diff the step's coding agent applies to the main worktree.
  - Touch nothing else in `c:\Users\Vinnie\cursor`.
  - New prose uses no em dashes and no double dashes, and it follows the Engine, Harness, and Host vocabulary that `crates/workshop/ui/test/docs-claims.mjs` checks.
  - Until the tidy ceiling is retired, it still binds the 23 crates whose crate doc carries `## Invariants`. A change that pushes one of their files past 500 lines splits the file first, under the split rule.
  - Doctest removal rule:
    - The scope is every compiled code block in a doc comment: untagged fences, fences tagged `rust`, `no_run`, `should_panic`, `compile_fail`, `ignore`, or an edition, and indented code blocks. `text`, `json`, and `toml` blocks stay.
    - Delete examples that only restate a signature, and delete every `compile_fail` case.
    - Move into the crate's tests, with its assertions, any example that exercises behavior no test covers.
  - Split rule:
    - Split a file only into private child modules of that file. Declare each one inside the file as `#[path = "<stem>-<topic>.rs"] mod <topic>;`, or lay it out as `<stem>/<topic>.rs`, choosing between the two forms by the flat-directory rule in `AGENTS.md`.
    - Each child module opens with a `//!` line naming its one concern.
    - Move an inline `#[cfg(test)]` module to `<stem>-tests.rs` first, when one exists.
    - Children reach the parent's private items through `super::`, and a moved item gets `pub(super)` only where the parent uses it.
    - Never widen anything to `pub(crate)` or `pub` to make a split compile. If no cohesive group splits out without widening, stop and report why.
  - Ceiling failure message:
    - Name the path, the line count, and the limit of 500.
    - State the split rule as `help:` lines.
    - End with the sentence "If no cohesive group splits out without widening visibility, stop and say why."
  - Ceiling wiring:
    - Wire a crate once all of its files are at or under 500 lines: add `build-ceiling.workspace = true` under `[build-dependencies]`, and give it a `build.rs` whose `main` calls `build_ceiling::check()`. A crate that already has a `build.rs` adds the call to it.
    - Six crates include the source instead and get no manifest entry: `harness`, `harness-gateway-client`, `harness-web`, `harness-runner`, `harness-capabilities`, and `promptforge-vfs`. Their `build.rs` declares `#[path = "<relative path>/build-ceiling/src/lib.rs"] mod build_ceiling;` and calls `build_ceiling::check()`.
      - `crates/build-xtask/src/product.rs` bars the Harness crates from every `build-*` crate in every dependency table.
      - `promptforge-vfs`'s `the_manifest_declares_no_dependencies` test refuses any build-dependency.
    - `build-ceiling`'s own `build.rs` includes its source the same way. If `unreachable_pub` fires on an included module, put `#[expect(unreachable_pub, reason = ...)]` on the `mod` declaration.
  - Visibility rule:
    - Give every `pub(crate)` and `pub(super)` item the narrowest of private, `pub(super)`, or `pub(crate)` that compiles across all targets and features.
    - Method: in each crate, narrow every such item to private, compile, then restore only the items the compiler reports as inaccessible, each at the narrowest level that compiles.
    - The facade surface in `crates/promptforge/public-api.txt` must not change.
  - The `promptforge-docs` layout:
    - Chapters go under `src/<book>/`, plus `src/introduction.md`, with `CONTRIBUTING.md` at the repository root.
    - `PROMPTFORGE_DOCS` names the checkout root. `build-user-guide` and `cargo xtask site` read `$PROMPTFORGE_DOCS/src/` wherever they read `guide/src/` before.
    - Both fail with a message naming the variable when it's unset or names no such directory.
    - Read the variable once in `main` and pass the path down, so tests need no `std::env::set_var`, which is unsafe in Rust 2024.
- Architecture:
  - File-size check: a new crate, `crates/build-ceiling`, has no dependencies (not even `workspace-hack`) and exposes `check()`. Every crate calls it from `build.rs`, wired under the ceiling wiring rule. Add `build-ceiling` to `[final-excludes] workspace-members` in `.config/hakari.toml`, so hakari never adds `workspace-hack` to it.
  - Cargo runs build scripts per package, so every crate needs its own call. A single script in a crate that everything depends on, such as `workspace-hack`, would rerun on every edit and rebuild the whole workspace.
  - `workspace-hack` is generated by `cargo hakari` (`.config/hakari.toml`) and gets no build script. A tidy check counts its files instead.
  - Lint policy: the root `[workspace.lints]` in `Cargo.toml` is the only lint table, and every crate declares `[lints] workspace = true`.
  - Docs site: `cargo xtask site` (`crates/build-xtask/src/site.rs`) builds the rustdoc sites listed in `RUSTDOC_SITES`, stages the books through `crates/build-user-guide`, writes the landing page, and checks every relative link. Chapter sources come from the directory named in `PROMPTFORGE_DOCS`. Site framework files stay in promptforge: `guide/landing/`, `guide/chrome/`, and `guide/books/<book>/book.toml`.
- Modules and interfaces:
  - `build_ceiling::check()`:
    - Finds the calling crate from `CARGO_MANIFEST_DIR`.
    - Counts every physical line with `str::lines().count()`, so blank, comment, and test lines all count, in every `.rs` file under `src/`, `tests/`, `benches/`, and `examples/`, plus `build.rs`.
    - Emits `cargo::rerun-if-changed` for those paths.
    - Has the signature `pub fn check() -> Result<(), build_ceiling::Violations>`, and never exits the process or panics.
    - `Violations` lists every file over 500 lines and every file the scan can't read, which it reports instead of skipping. Its `Display` and `Debug` both render the ceiling failure message, because a `main` that returns a `Result` prints its error through `Debug`.
    - Each build script's `main` turns the error into a failed build:
      - A new `build.rs` is `fn main() -> Result<(), build_ceiling::Violations> { build_ceiling::check() }`.
      - The four existing build scripts call `check()` first in `main`. `crates/gateway/app` and `crates/workshop/desktop` use `?`, because their `main` returns a `Result`. `crates/gateway/config-ui` and `crates/workshop/server` print the error and return `ExitCode::FAILURE`, because their `main` returns an `ExitCode`.
  - New tidy checks in `crates/build-xtask`, run by `cargo test -p build-xtask` and `cargo xtask tidy`:
    - Lint inheritance: every workspace crate declares `[lints] workspace = true`.
    - Unsafe allowlist: `unsafe_code` is expected or allowed, directly or through `cfg_attr`, only in `crates/gateway/app`, `crates/gateway/stt/whisper-ffi`, `crates/gateway-api-discovery`, and `crates/workshop/desktop`.
    - Ceiling wiring: every workspace crate's `build.rs` calls `build_ceiling::check()` through either form in the ceiling wiring rule. The two exceptions are `build-ceiling`, whose `build.rs` includes its own source, and `workspace-hack`, whose `.rs` files the check counts against the same 500-line limit.
    - No doctests:
      - It sits beside `crates/build-xtask/src/doc_hidden.rs` and uses `syn` the same way.
      - It reads every doc attribute (`///`, `//!`, `#[doc = "..."]`) in every workspace crate, classifies code blocks as rustdoc does with `pulldown-cmark` (already in `[workspace.dependencies]`), and fails on any compiled block under the doctest removal rule's scope and on any `doc = include_str!`.
      - Its message says to move the example into a test.
      - String literals in `crates/build-xtask`'s own test fixtures are data, not doc attributes.
    - Each check fails when it scanned nothing.
  - `cargo xtask new-crate` (`crates/build-xtask/src/new_crate.rs`) writes the `build.rs` and the build-dependency into every new crate.
  - Landing generation:
    - Each `RUSTDOC_SITES` entry gains a short, broad description.
    - Each `guide/books/<book>/book.toml` gains a `description` under `[book]`.
    - `cargo xtask site` writes `target/site/index.html` from both lists, so a new rustdoc site or book adds its row automatically.
- File and public API changes:
  - The `promptforge` facade surface (`crates/promptforge/public-api.txt`) changes only when a visibility change reaches it.
    - Regenerate it with `cargo +<pinned nightly> xtask api --bless`, and confirm with `--check`.
    - The pinned nightly is the one named in `crates/build-xtask/src/api/toolchain.rs`.
  - `[workspace.lints.rust]`:
    - Lower `unsafe_code` from `forbid` to `deny`, because a `forbid` can't be relaxed by a module-level `expect`, which is what forced four crates to copy the lint table.
    - Keep every other existing entry.
  - `[workspace.lints.clippy]`:
    - Add `undocumented_unsafe_blocks`, `missing_safety_doc`, `allow_attributes`, `allow_attributes_without_reason`, `exit`, `disallowed_methods`, `disallowed_types`, and `disallowed_macros`, all at `deny`.
    - `pedantic` stays at `deny`.
  - Root `clippy.toml` gains `disallowed-methods` entries for the process-global installers:
    - `std::panic::set_hook`, `log::set_logger`, `log::set_boxed_logger`, and `tracing::subscriber::set_global_default`.
    - `tracing_subscriber::util::SubscriberInitExt::init` and `try_init`.
    - `rustls::crypto::CryptoProvider::install_default`.
    - Each reason reads "use X instead, because Y". Add `replacement` only where the call shape matches, and drop any entry whose crate isn't in `Cargo.lock`.
  - `deny.toml` gains a `[[bans.features]]` entry: `crate = "serde_json"`, `deny = ["preserve_order"]`.
  - `tools/cicerone.md` and `tools/cicerone/` (two plans and ten Python scripts) move to `promptforge-design/saved-docs/cicerone/`. Nothing outside `vibe/` references them.
  - `.cargo/config.toml` gains `[env] CLIPPY_DISABLE_DOCS_LINKS = "1"`.
  - Warnings as errors:
    - Gate commands (CI and `.githooks/pre-push`) set `CARGO_BUILD_WARNINGS=deny` instead of passing `-- -D warnings`.
    - The gate never sets `RUSTFLAGS`. On Windows it replaces the `+crt-static` setting in `.cargo/config.toml`, and anywhere it changes compiler flags and throws away the build cache.
    - The setting does not go in `.cargo/config.toml`. On the unpinned stable channel, a new Rust release that adds a lint then fails only the gate, not every local build.
  - The `AGENTS.md` policy line that replaces the explicit-approval rule reads: "Gate on yes/no properties, never on a score threshold that ordinary work edits." A single fixed bound in reviewed code, such as the 500-line limit, counts as a property.
- Data, persistence, failure, security, and privacy constraints:
  - JSON that reaches a recorder or a replay comparison keeps exact round-trips, so serde_json `preserve_order` stays off. cargo-deny enforces that.
  - A `build.rs` failure also fails release and installer builds, so CI must catch a violation first.
  - New and edited prose follows the Engine, Harness, and Host vocabulary rules in `AGENTS.md`, which `crates/workshop/ui/test/docs-claims.mjs` checks.

</implementation-contract>
<verification-contract>

## Testing Plan

Every new check gets behavior tests built on fixture crates, in the style of `crates/build-xtask/src/tidy-tests.rs`. Moved examples become ordinary tests that keep the results their doctests asserted. The full Project Survey command set must pass at the end. Removing doctests is the main speed gain: before this plan, the doctest steps added about 67 seconds to every local gate run.

- Unit:
  - `build-ceiling` counts every physical line, including blank lines, comments, CRLF endings, and a missing final newline. A 500-line file passes and a 501-line file fails. The failure message includes the path, the count, the limit, the split rule, and the stop sentence. An unreadable file is reported, not skipped.
  - Each new tidy check has a passing fixture and a failing fixture, and fails when it scans nothing. The ceiling-wiring check passes both wiring forms. The no-doctests check fails on an untagged fence, a `rust` fence, an indented code block, and a `doc = include_str!`, and passes on `text`, `json`, and `toml` fences.
  - Landing generation (`crates/build-xtask/src/site-tests.rs`) gives every `RUSTDOC_SITES` entry and every staged book exactly one row.
  - `build-user-guide` reads chapters from `PROMPTFORGE_DOCS` (`crates/build-user-guide/src/stage-tests.rs`).
- Integration and end-to-end:
  - The full suite, clippy, docs, facade docs, `xtask api --check`, `cargo deny check`, and `cargo test -p build-xtask` all pass.
  - `cargo xtask site` builds with `PROMPTFORGE_DOCS` pointing at a `promptforge-docs` checkout, and its link check passes.
  - A deliberately oversized scratch file fails a crate's build with the specified message, and the build passes again once it's removed.
- Regression, security, and performance:
  - Ported tutorial examples keep their asserted results. For example, the complete greeter program still returns `HI THERE`.
  - `cargo xtask api --check` confirms the facade surface changed only where the visibility work intends.
  - The doctest baseline, measured on 2026-10-03 on the development machine after a typical edit:
    - The main partition ran 303 doctests in about 63 to 67 seconds. That cost was fixed, the same with nothing changed, and it scales with the number of crates, not the number of doctests.
    - The workshop partition ran zero doctests in 2 seconds.
    - Nextest took about 96 seconds.
- Exit criteria:
  - Every Project Survey command passes.
  - No `doc = include_str!` remains in any crate, and no compiled doctest remains in any doc comment.
  - No `.rs` file in the workspace exceeds 500 lines.
  - A temporary 501-line `.rs` file fails `cargo build` with the ceiling failure message, in one dependency-wired crate and one `#[path]`-wired crate.
  - `cargo test -p build-xtask` fails on each probe:
    - `unsafe_code` expected outside the four allowed crates;
    - a crate without `[lints] workspace = true`;
    - a crate whose `build.rs` doesn't call `build_ceiling::check()`;
    - a `workspace-hack` file over 500 lines;
    - a compiled code block in a doc comment.
  - Turning on `serde_json/preserve_order` fails `cargo deny check`, and a `std::panic::set_hook` call in a library crate fails clippy.
  - Revert every probe.
  - With `PROMPTFORGE_DOCS` set, `cargo xtask site` builds, writes the seven-row landing table, and passes its link check.
  - `tools/cicerone` no longer exists in promptforge.

</verification-contract>
<decision-record>

## Decision Record

The user settled each decision below in conversation, and their words are quoted where they set the rule. The theme is to trade documentation for enforcement, and to keep encapsulation ahead of file size. Rejected alternatives record why each failed and when to revisit it.

- Decisions:
  - Only doc comments document code inside promptforge: "The ONLY docs in the repo will be doc comments". Rustdoc stays in promptforge because "promptforge repo is the source of truth for the Rustdocs".
  - Every markdown file pulled into rustdoc is removed and archived: "I want all lib.md and related markdown includes removed from promptforge and archived in the promptforge-design repo under a directory in saved-docs".
  - Doctests go, because removing them speeds up the gate. Each one is moved or deleted at the implementer's discretion: "for each doctest either move it or delete it (your call)".
  - READMEs become generic: "rewritten to be so generic they can never go out of date. Just describe the crate's responsibility in broad terms. Sand off anything that could go stale." The root README keeps its images and badges: "keep the badges. lose anything related to crates.io - we don't publish there."
  - The landing page becomes "a simple table. No images", and the Workshop guide keeps a row. It is generated from the rustdoc and book lists because the hand-written page already missed `harness-gateway-client` as a top-level crate.
  - All crates.io metadata goes ("I dont publish to crates.io and I never intend to"), but installers must not break ("I dont want installers to break"). So `description` stays on the two shipped binaries.
  - The three guides move to `promptforge-docs`, but the framework stays: "The guides (3 exposition guides) should be moved yes but not the framework (landing page). The CI for promptforge still needs to build the overall github pages site". No history comes with them, and the site rebuilds on a nightly schedule.
  - One lint table: "I do not want copies of workspace lint. I want the strictest possible settings that still lets us vibe." `unsafe_code` drops to `deny` because `forbid` can't be relaxed per module. The unsafe allowlist check restores the old strictness for the other 45 crates.
  - The explicit-approval rule for structural checks is removed from `AGENTS.md`. Where `AGENTS.md` conflicts with an adopted rule, the conflicting part is deleted, and shrinking beats expanding.
  - Gate on yes/no properties, never on an editable score threshold. The 500-line limit is one fixed constant in reviewed code, so it counts as a property.
  - The 500-line limit applies everywhere with no exceptions, is enforced at compile time, and must not cost encapsulation:
    - "the 500 line limit should be enforced everywhere, no exceptions"
    - "encapsulation for me is more important than the file size ... encapsulation prevents entanglement"
    - When the limit is exceeded, the build "should generate a compile error, and it should explain the problem, and it should offer the solution."
    - It is the last work item.
  - The gate turns warnings into errors with `CARGO_BUILD_WARNINGS=deny`, not in `.cargo/config.toml`, so a new stable Rust lint can't break every local build mid-session.
  - Six crates wire the 500-line check without naming `build-ceiling` in their manifests (added during decomposition, 2026-10-03):
    - The product-boundary matrix in `crates/build-xtask/src/product.rs` bars the five Harness crates (`harness`, `harness-gateway-client`, `harness-web`, `harness-runner`, `harness-capabilities`) from every `build-*` crate in every dependency table, build-dependencies included.
    - `promptforge-vfs`'s `the_manifest_declares_no_dependencies` test refuses any build-dependency, and its manifest says never to weaken that rule.
    - So these six crates' `build.rs` files pull in `crates/build-ceiling/src/lib.rs` with `#[path]` and call `build_ceiling::check()`, the way `build-ceiling`'s own `build.rs` does. Both existing rules stay unchanged, and the ceiling-wiring check accepts either form.
    - Rejected: exempting `build-ceiling` from both rules. It weakens a product boundary, and a rule its owner marked never to weaken, for the sake of tooling.
    - The user confirmed this choice when asked during review.
  - `tools/cicerone` is archived to `promptforge-design/saved-docs/cicerone/`. It generated the rustdoc tutorial pages this plan deletes, and its instructions still target `lib.md` and `guide/src/`, so an agent could regenerate them. The user chose archiving during review.
  - A tidy check rejects new doctests, because once the doctest steps are gone, a doctest an agent writes later would never run. The user chose adding it during review.
  - The run does everything, pushes included: "YOU DO EVERYTHING". It pushes the two sibling repositories as it commits to them, and it pushes promptforge and checks CI and the site workflow at the end.
  - The run uses nine steps, not 29, because each step's fixed cost of review, verification, and commit message dominated: "do we REALLY need 29 steps?" The steps group work by subject, and the three largest (the doctest sweep, the file splits, and the visibility pass) fan out to parallel sub-agents inside the step.
  - `promptforge-docs` keeps the old `guide/` layout (added during decomposition, 2026-10-03): chapters under `src/<book>/`, plus `src/introduction.md`, with `CONTRIBUTING.md` at its root. `PROMPTFORGE_DOCS` names the root of that checkout, and `build-user-guide` and `cargo xtask site` read `$PROMPTFORGE_DOCS/src/` where they read `guide/src/` before. Both fail with a message naming the variable when it is unset or names no such directory.
  - `build_ceiling::check()` returns `Result<(), build_ceiling::Violations>` instead of exiting the process (added during decomposition, 2026-10-03):
    - Library code never exits the process, and the workspace's `clippy::exit` deny rejects `process::exit` outside `main`, so the check reports through its return value and the build script's `main` turns it into a nonzero exit.
    - `Violations` lists every oversized or unreadable file. Its `Display` and `Debug` both render the ceiling failure message, because a `main` that returns a `Result` prints its error through `Debug`.
    - A new `build.rs` is `fn main() -> Result<(), build_ceiling::Violations> { build_ceiling::check() }`. The four existing build scripts call it first in `main`: with `?` in `crates/gateway/app` and `crates/workshop/desktop`, whose `main` returns a `Result`, and by printing the error and returning `ExitCode::FAILURE` in `crates/gateway/config-ui` and `crates/workshop/server`, whose `main` returns an `ExitCode`.
    - Rejected: calling `process::exit` under `#[expect(clippy::exit)]`, which breaks the rule that only binary entry points exit; and panicking, which buries the message under a panic header.
- Rejected alternatives:
  - Dylint for the file-size check:
    - It needs a pinned nightly with `rustc-dev`.
    - It runs as a separate type-check pass that shares no cache with stable builds.
    - Counting lines needs none of its name resolution.
    - Revisit only for semantic rules; see Deferred.
  - A Cargo `rustc-wrapper` for a single central check:
    - The wrapper would have to be installed on every machine and CI job before any build could run.
    - `cargo clippy` overrides `rustc-workspace-wrapper`.
    - Revisit if Cargo gains workspace-level build scripts.
  - One build script in `workspace-hack`: it would rebuild the whole workspace on every edit. No revisit.
  - Keeping the ceiling only as a tidy check, a warning, or nothing:
    - The user wants a compile error.
    - Crates outside today's check grew 66 files past 500 lines, and one file went from 473 to 574 lines in a day after leaving the check's scope.
    - No revisit.
  - A per-file ratchet or freeze-and-shrink baseline: this repository ran one in `module-ceilings.toml`. Its limits were raised 186 times and lowered 56 before it was removed on 2026-09-08 (`vibe/2026-09/2026-09-08-1-remove-unsupported-ratchets.md`). No revisit.
  - Splitting files into sibling modules with widened visibility: it loosens encapsulation, as when one split widened `Attached` to `pub(crate)`. Private child modules achieve the same split with no widening. No revisit.
  - Showing tested examples in docs by including test files with `include_str!`: readers are not a goal. Revisit if outside readers become a goal.
  - Deleting all docs, or keeping rustdoc in a separate repository: rustdoc must stay with the code. No revisit.
  - An orphan branch for the guides: the user chose the existing `promptforge-docs` repository. No revisit.
  - Rebuilding on every docs push with `repository_dispatch`: it needs a cross-repository token. Revisit if a one-day publish lag becomes a problem.
  - A `.cursorignore` for `guide/`: the guides leave the repository, which makes it moot. No revisit.
  - `build.warnings = "deny"` in `.cargo/config.toml`: on the unpinned stable channel, a new lint would break every local build. Revisit if the toolchain gets pinned.
  - Hand-copied lint tables: the user rejected them. No revisit.
  - Banning `std::thread` spawns in Harness crates:
    - Harness crates have none.
    - Harness crates already may not depend on tokio at all (`crates/build-xtask/src/harness_bans.rs`).
    - Enforcing it would need per-crate `clippy.toml` copies.
    - Revisit if a Harness crate starts threads.
  - candor-rust, cargo-pup, checks over rust-analyzer's SCIP index, Trustfall, and Vale: immature, or untested on Windows. Revisit when one matures.
- Assumptions, risks, and notes:
  - Size checks cover `.rs` files only. TypeScript and CSS stay out.
  - Cargo applies `[env]` to the compiler processes it runs. That should make `clippy-driver` honor `CLIPPY_DISABLE_DOCS_LINKS`, but this is unverified, so confirm that the docs-link line disappears.
  - `CARGO_BUILD_WARNINGS` might not cover rustdoc warnings, so rustdoc steps keep `RUSTDOCFLAGS="-D warnings"` unless that's verified. Cargo ignores `build.warnings` under `--message-format=json`, which rust-analyzer uses, so the editor still shows warnings.
  - Whether the Tauri bundler or cargo-dist reads `description` is unverified. That's why it stays on both shipped binaries.
  - `promptforge-docs` sits at `c:\Users\Vinnie\cursor\promptforge-docs`, inside the workspace folder. An agent opened on `c:\Users\Vinnie\cursor` can still read the guides there, so vibe with `promptforge` opened as its own workspace. The repository is public (`git@github.com:cppalliance/promptforge-docs`, default branch `master`) and holds only `README.md` and `LICENSE`.
  - Edits to `promptforge-design` and `promptforge-docs` are committed in those repositories, separately from promptforge's commits.
  - `core.hooksPath` is a per-clone git setting, and this plan sets it only in this clone. The hooks are bash scripts, which Git for Windows runs.
  - The 500-line limit as of 2026-10-03:
    - The tidy ceiling covered 23 of 49 crates. 0 of their 751 files are over 500 lines.
    - 66 files elsewhere are over. The longest is `crates/gateway/local/src/runtime.rs` at 2,127 lines.
    - `crates/workshop/workspace/src/workspace/backing.rs` is at exactly 500, which passes.
  - The visibility pass covers about 3,300 `pub(crate)` and `pub(super)` uses in about 490 files, which makes it the largest single body of work.
  - The tidy ceiling's "stays under 500 lines" bullet appears in 23 crates' `## Invariants` docs. The workspace has 4 real `#[allow]` attributes; the other `#[allow]` text is fixture data in `crates/build-xtask` tests.
  - A build script in every crate has costs:
    - Cargo prints a generic "failed to run custom build command" header above the message.
    - rust-analyzer's view of a crate degrades while one of its files is over the limit.
    - Each clean build compiles 45 extra small build-script programs.
    - The four existing build scripts already emit narrow `rerun-if-changed` lines. Once the ceiling adds its `src/`, `tests/`, `benches/`, and `examples/` lines, every source edit in `gateway`, `gateway-config-ui`, `workshop-server`, or `workshop` reruns its whole script: the icon resources, esbuild, or the Tauri build. Step 8 measures that cost.

### Deferred and Out of Scope

- Deferred: a Dylint lint that flags `pub(crate)` and `pub(super)` items wider than their uses. Revisit after this plan, and time its gate cost.
- Deferred: an ast-grep rule that workaround comments cite an upstream issue URL. Revisit once ast-grep is adopted, then delete that `AGENTS.md` rule.
- Deferred: a `cargo xtask fix-layout` generator for the flat-directory convention. Revisit once the generator exists, then delete the `AGENTS.md` flat-directory paragraph.
- Deferred: a unified `cargo xtask check` command, Cursor hooks (a stop hook, a write guard, a shell guard), and rustc-style xtask messages. Revisit after this plan.
- Deferred: a scheduled job in `promptforge-docs` that parses every prompt example against promptforge's current parser and reports drift. Revisit once the guides have moved.
- Deferred: hiding old plans under `vibe/` from agents. Revisit when stale plans mislead a session.
- Out of scope: file-size limits for TypeScript and CSS.
- Out of scope: publishing any crate to crates.io.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the workspace default member). The desktop app builds only on request: `cargo build --locked -p workshop`. The headless gateway shape: `cargo check -p gateway --no-default-features`.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-substring>`; for `workshop`, `workshop-server`, and `workshop-server-api`, drop `--all-features`. A UI test file: `node --test <file>.mjs` from its package directory under `crates/workshop/` (`ui`, `look`, or `platform`).
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features` (same `--all-features` exception for the three workshop crates). Structural checks: `cargo test -p build-xtask` (on demand: `cargo xtask tidy`). A UI package: `npm test --workspace <ui|look|platform>` from `crates/workshop`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`. UI: `npm test --workspaces --if-present` from `crates/workshop`, and `npm test` from `crates/gateway/config-ui/ui`. Nightly-only fixtures: `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`; never a standalone `cargo check --workspace` beside them. Also `cargo deny check` (pre-push and CI), and UI typechecks `npm run typecheck --workspaces --if-present` from `crates/workshop` and `npm run typecheck` from `crates/gateway/config-ui/ui`.
- Formatter check command: `cargo fmt --all --check` (also the pre-commit hook).
- Docs command: with `RUSTDOCFLAGS=-D warnings` set (PowerShell: `$env:RUSTDOCFLAGS="-D warnings"`), `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then the default-feature facade docs `cargo doc -p promptforge --no-deps` and `cargo doc -p harness --no-deps`. User guide: with `PROMPTFORGE_DOCS` set to `c:\Users\Vinnie\cursor\promptforge-docs` (PowerShell: `$env:PROMPTFORGE_DOCS="c:\Users\Vinnie\cursor\promptforge-docs"`; ignored until the guides move), `cargo xtask site --books-only`. Facade surface: `cargo +nightly-2026-09-05 xtask api --check` (the pin lives in `crates/build-xtask/src/api/toolchain.rs`).
- Test placement and naming conventions:
  - Unit tests sit in `#[cfg(test)] mod tests` inline (about 380 files), or in a sibling `foo-tests.rs` wired as `#[cfg(test)] #[path = "foo-tests.rs"] mod tests;`, or in a `src/<module>/tests/` directory once there are three or more test files.
  - Integration tests are one binary per crate: `crates/<crate>/tests/it/main.rs` or `tests/suite/main.rs`, with one module per topic beside it plus a `support.rs`.
  - Test functions are snake_case behavior sentences, such as `a_harness_crate_without_the_marker_is_a_violation_and_still_held_to_the_ceiling`.
  - Structural-check tests build fixture crates in the style of `crates/build-xtask/src/tidy-tests.rs`.
  - Criterion benches use `harness = false` in `promptforge-engine` and `promptforge-lua`.
  - UI tests are `node --test` `.mjs` files in `crates/workshop/<pkg>/test/`, plus `src/**/*.test.mjs` in `ui`. `crates/workshop/ui/test/docs-claims.mjs` checks the Engine, Harness, and Host vocabulary in every `AGENTS.md`, `## Invariants` crate doc, and `.cursor/rules` file.
- Directory map:
  - `crates/`: every Rust crate. The Engine (`promptforge`, `promptforge-internal/*`), the Harness (`harness*`, `harness-internal/*`), the gateway family (`gateway/*`, with STT under `gateway/stt/*`), the Workshop family (`workshop/*`), shared crates (`gateway-api-types`, `gateway-api-discovery`, `shared-loopback`, `shared-error-source`), build tooling (`build-xtask`, `build-ui`, `build-workshop`, `build-user-guide`, `build-llama-cuda`), and `workspace-hack` (cargo-hakari). `crates/shared-ui` is a TypeScript and CSS package, not a Rust crate.
  - `guide/`: the user guide. Chapter sources are in `src/` and the long-form `*-guide.md` files, with `books/*/book.toml`, `landing/`, and `chrome/` for the site.
  - `prompts/`: example prompt programs. `local/`: machine-local gateway config, profiles, prompts, and STT fixtures.
  - `tools/`: Node sidecar-staging and TTS scripts with tests, plus the `cicerone` Python tooling.
  - `vibe/`: plans (older ones in `vibe/2026-09/`) and `archdoc.md`.
  - `.github/workflows/`: CI, release, nightly, site, and Miri jobs. `.githooks/`: pre-commit fmt and pre-push clippy and deny.
  - `.cargo/config.toml`: the `xtask` and `workshop` aliases, and rust-lld with a static CRT on Windows. `.config/`: nextest and hakari config. `.cursor/rules/`: agent rules.
  - Root config: `Cargo.toml`, `clippy.toml`, `deny.toml`, `rustfmt.toml`, `rust-toolchain.toml` (stable), and `dist-workspace.toml` (cargo-dist). `target/` and `target-msrv/` are build output.
- Component boundaries:
  - The Engine depends on the Lua VM boundary and the shared substrate.
  - The Harness depends only on the Engine, through the `promptforge` facade. It has no gateway or shared crate, and no tokio in Harness crates.
  - The Host (Workshop, the CLI) depends on the Harness.
  - The gateway family depends on the shared substrate.
  - The Workshop family depends on the Harness, the gateway, and the shared substrate, in a one-way tier graph.
  - The VFS layer depends on nothing.
  - Build crates are tooling and depend on no workspace crates.
  - `cargo test -p build-xtask` enforces the product and container boundaries, the Workshop tier graph, lint inheritance, the `## Invariants` marker, and the 500-line file ceiling. `cargo xtask api --check` keeps the `promptforge` facade surface closed against `crates/promptforge/public-api.txt`.
- Conventions summary:
  - Rust 2024 on stable. Dependencies are centralized in `[workspace.dependencies]`, with a comment justifying each pin.
  - One workspace lint table: clippy `all` and `pedantic` deny, `unwrap_used` and `expect_used` deny, `unsafe_code` forbid outside the owned unsafe boundaries, `missing_docs` warn, and rustdoc broken links deny.
  - Source directories are flat, with kebab `foo-bar.rs` siblings wired by `#[path]` until a group reaches three files. Files stay at or under 500 lines.
  - Engine, Harness, and Host are capitalized defined terms.
  - Error messages are written for a model to read. Comments explain constraints and cite upstream issue URLs for workarounds.
  - Recorded JSON round-trips exactly (`float_roundtrip`, sorted keys, never `preserve_order`).
  - Workshop CSS uses `--ws-*` tokens, and the SPA never uses `localStorage`.
  - Behavior changes ship with tests, and CI commands pass `--locked`.

</project-survey>
<execution-plan>

## Execution Instructions

The work runs as 9 steps in four components. Each step is one promptforge commit holding its code and its tests. A step that also changes `promptforge-design` or `promptforge-docs` commits there first, as a separate commit, and pushes that repository right away. The run does every push itself: it pushes the two sibling repositories as it commits to them, and the last step pushes promptforge after full verification, starts the site workflow, and watches that run and the CI run until both succeed. The three largest bodies of work (the doctest sweep in Step 2, the file splits in Step 7, and the visibility pass in Step 9) fan out to parallel sub-agents on disjoint files inside their step, and the step's coding agent merges the results and runs the step's checks. Work item numbers refer to the 13 todos in the frontmatter.

- Rules for every step:
  - Find each edit by its quoted text. Line numbers and line counts below are as of 2026-10-03 and drift.
  - Verify with the Project Survey commands for the crates the step touches, plus `cargo fmt --all --check` and the checks the step names. Each step repeats this in its own checks.
  - Remove retired files with `git rm`; the history keeps them. Touch nothing in `c:\Users\Vinnie\cursor` outside `promptforge`, `promptforge-design`, and `promptforge-docs`.
  - Write new prose without em dashes or double dashes, in the Engine, Harness, and Host vocabulary that `crates/workshop/ui/test/docs-claims.mjs` checks.
  - Until Step 8 removes it, the tidy ceiling still binds the 23 crates with the `## Invariants` marker. A step that grows one of their files past 500 lines splits it under the split rule in Technical Design.
  - Sub-agents never stage or commit, and the session running the step commits and pushes the sibling repositories, as the rules in Technical Design set out.
- Components, in dependency order:
  1. Docs out of rustdoc (Steps 1 and 2; items 1 and 2). First, because item 2 needs item 1 done, and dropping the doctest steps takes about 67 seconds off every later local gate run.
  2. READMEs, manifests, and docs site (Steps 3 and 4; items 3, 4, 5, and 6). Independent of the enforcement work. Early, so the steps that later edit the same manifests (the lint tables in Step 5, the build-dependencies in Step 8) start from trimmed files, and so the Docs line of `AGENTS.md` is final before Step 6 shrinks the file.
  3. Structural enforcement (Steps 5 and 6; items 7 to 12). Before the ceiling, because item 13 depends on every other item.
  4. Compiler-enforced 500-line limit (Steps 7 to 9; item 13). Last, because its splits touch files every other component edits, its last step verifies the whole plan and pushes promptforge, and the user asked for it last.
- Pieces and construction:
  - Docs out of rustdoc: the rustdoc pages, their ported tests and examples, and the generator that wrote them (Step 1), then every remaining doctest, the no-doctests tidy check, and the removal of the doctest gate (Step 2). Sequential, because only once the pages are gone are the remaining doctests the per-symbol ones, and the new tidy check rejects every `doc = include_str!` that Step 1 removes. Inside Step 2 the tidy check comes first, because its report is the complete list of blocks to remove: the two doctest commands never see binaries, build scripts, or integration tests. The family sub-agents then touch disjoint crates, so their order is free, and the gate removal follows their merge.
  - READMEs, manifests, and docs site: the READMEs and the manifests (Step 3), then the landing page and the guide move (Step 4). The two steps read nothing of each other, so their order is free. Inside Step 3 the READMEs and the manifests are joint, because neither reads the other. Inside Step 4 the landing page comes before the guide move, because both edit the site build and the move builds on the generated page.
  - Structural enforcement: stale config, the lint table, and the strict clippy lints (Step 5), then the bans, the gate settings, and `AGENTS.md` (Step 6). Sequential: the strict lints need every crate on the workspace table; the bans need the per-crate `clippy.toml` deletions and the `disallowed_methods` deny from Step 5; and the `AGENTS.md` edits delete the prose that the lint table, the strict lints, the bans, and the gate settings replace.
  - Compiler-enforced 500-line limit: the file splits (Step 7), then the `build-ceiling` crate, its wiring into every crate with its tidy check, and the retirement of the tidy ceiling (Step 8), then the visibility pass, the final verification, the push, and the CI and site runs (Step 9). Sequential: a wired crate with a file over 500 lines fails its build, so every split comes before the wiring; the tidy ceiling retires only once the compile-time check binds every crate; the visibility pass follows the splits so it covers the new child modules, and follows the wiring so it covers `build-ceiling`; and the final verification and the push come last.

<step-1>

### Step 1: Archive the rustdoc pages and cicerone [completed]

- Component: Docs out of rustdoc
- Work item: 1, for `promptforge`, `promptforge-engine`, `harness`, and `tools/cicerone`.
- Archive:
  - Copy the 13 pages in `crates/promptforge/src/` (`lib`, `effect`, `event`, `ids`, `model`, `tools`, `capabilities`, `prompt`, `vfs`, `cancel`, `timestamp`, `metrics`, and `replay`, each `.md`) to `promptforge-design/saved-docs/promptforge/`.
  - Copy `crates/promptforge-internal/engine/src/lib.md` to `promptforge-design/saved-docs/promptforge-engine/`.
  - Copy `crates/harness/src/{lib,capability,record,vfs}.md` to `promptforge-design/saved-docs/harness/`.
  - Copy `tools/cicerone.md` and `tools/cicerone/` (two plans and ten Python scripts) to `promptforge-design/saved-docs/cicerone/`. Cicerone wrote the rustdoc pages this step deletes, and its instructions still target `lib.md` and `guide/src/`. Nothing outside `vibe/` references it.
  - Commit the copies in `promptforge-design` (`c:\Users\Vinnie\cursor\promptforge-design`) and push that repository right away. Then `git rm` the 18 pages and both cicerone paths from promptforge.
- Tests:
  - Port into `crates/promptforge/tests/suite/`, one topic module per page wired from `tests/suite/main.rs`, every `promptforge` page example whose behavior no suite test covers, with its assertions.
  - Port the engine page's version-detection and parse example into the engine's `tests/` binary if no test covers it.
  - Port into `crates/harness/tests/suite/`, wired from `tests/suite/main.rs`, every `harness` page example whose behavior no suite test covers, with its assertions. The candidates are the tours that answer a run's question through an input broker, stop a round and cancel a run, and stream from a Host's own broker.
- Examples:
  - Add `crates/promptforge/examples/greeter.rs`, the canonical step-and-answer greeter loop from the `promptforge` page's complete program. A suite test asserts that the complete greeter returns `HI THERE`.
  - Add `crates/harness/examples/run-prompt.rs`, the canonical example from the `harness` page's "Run a prompt" tour, using only the crate's existing dev-dependencies (`tokio`, `async-trait`).
- Docs:
  - In `crates/promptforge/src/lib.rs` and `crates/promptforge-internal/engine/src/lib.rs`, remove the 14 `#![doc = include_str!(...)]` lines and write a short `//!` crate doc. Give each `pub mod` block in the facade that had a page a one-line module doc. Keep the engine's `## Invariants` section. New doc text names no internal crate, because `xtask api --check` rejects that in facade docs.
  - In `crates/harness/src/lib.rs`, remove the four `include_str!` lines, write a short `//!` crate doc, and give the `capability`, `record`, and `vfs` module blocks one-line docs.
- Checks:
  - The Project Survey commands for `promptforge`, `promptforge-engine`, and `harness`, plus `cargo fmt --all --check`.
  - `cargo nextest run --locked -p promptforge -p promptforge-engine -p harness --all-features`.
  - `cargo run --locked -p promptforge --example greeter` and `cargo run --locked -p harness --example run-prompt`.
  - With `RUSTDOCFLAGS=-D warnings`: `cargo doc -p promptforge --no-deps`, `cargo doc -p promptforge-engine --no-deps --all-features`, and `cargo doc -p harness --no-deps`.
  - `cargo +nightly-2026-09-05 xtask api --check` and `cargo test -p build-xtask`.
  - `rg 'doc = include_str!' crates` matches only the fixture strings in `crates/build-xtask/src/*-tests.rs`, so it finds nothing in `promptforge`, `promptforge-engine`, or `harness`.
  - `tools/cicerone.md` and `tools/cicerone/` no longer exist, and `rg -i cicerone --glob '!vibe/**'` finds nothing.
  - `git -C c:\Users\Vinnie\cursor\promptforge-design status -sb` shows no unpushed commit.

</step-1>

<step-2>

### Step 2: Remove every doctest and replace the doctest gate

- Component: Docs out of rustdoc
- Work item: 2, for every crate with a compiled doc-comment block, then the tidy check, then the gate.
- Order: write the tidy check and its tests first, then run it on the live workspace. Its report, not the two doctest commands, is the complete list of blocks to remove, because `cargo test --doc` never sees binaries, build scripts, or integration tests. Divide the reported crates among parallel sub-agents by family (Engine and Harness; gateway and shared; Workshop), each on disjoint files, and give any reported crate outside the families to the coding agent. Once their changes are merged and the check passes on the live workspace, remove the gate.
- Apply the doctest removal rule in Technical Design:
  - Scope: every compiled code block in a doc comment, meaning untagged fences, fences tagged `rust`, `no_run`, `should_panic`, `compile_fail`, `ignore`, or an edition, and indented code blocks. `text`, `json`, and `toml` blocks stay.
  - Delete examples that only restate a signature, and every `compile_fail` case. Move into the crate's tests, with its assertions, any example that exercises behavior no test covers.
- Families, each covering every crate in it that has a compiled block:
  - Engine and Harness: `promptforge-types`, `promptforge-engine`, `promptforge-lua`, `promptforge-parser`, `promptforge-model-client`, `promptforge-vfs`, `harness-runner`, `harness-capabilities`, `harness-gateway-client`, and `harness-web`. The most blocks are in `crates/promptforge-internal/types/src/tools/output.rs`, `crates/harness-internal/capabilities/src/capability.rs`, and `crates/harness-gateway-client/src/transport.rs`.
  - Gateway and shared: every package under `crates/gateway/`, plus `gateway-api-types`, `gateway-api-discovery`, and `shared-loopback`. Most blocks are in `gateway-config` (`crates/gateway/config/src/config/accessors.rs` alone holds about 70, then `config/stt.rs`, `shadow.rs`, and `profile.rs`), then `crates/gateway-api-types/src/metadata.rs`, `gateway-logging`, and `crates/gateway/local/src/runtime.rs`.
  - Workshop: `workshop-agents`, `workshop-gateway`, and `workshop-support`, as of 2026-10-03.
  - After the merge, the coding agent removes, the same way, any compiled block the tidy check still reports, such as one in a `build-*` crate or a binary.
- Tidy check, so a doctest an agent writes later fails a check instead of never running:
  - New `crates/build-xtask/src/no_doctests.rs`, declared in `main.rs` beside `doc_hidden`, exposes `no_doctests_violations(root)`, which `all_violations` in `crates/build-xtask/src/tidy.rs` calls.
  - It scans every workspace crate, parsing each `.rs` file with `syn` the same way `doc_hidden.rs` does. It reads every doc attribute (`///`, `//!`, `#[doc = "..."]`), classifies code blocks as rustdoc does with `pulldown-cmark` (already in `[workspace.dependencies]`), and reports the file and line of every compiled block under the doctest removal rule's scope and of every `doc = include_str!`.
  - Its message says to move the example into the crate's tests.
  - String literals in `crates/build-xtask`'s own test fixtures are data, not doc attributes, so they never trip it. It fails when it scanned nothing.
  - Add `pulldown-cmark.workspace = true` under `[dependencies]` in `crates/build-xtask/Cargo.toml`. If `cargo hakari verify` then fails, run `cargo hakari generate` and commit the result in this step.
- Tests in `crates/build-xtask/src/no_doctests-tests.rs`, wired as `#[cfg(test)] #[path = "no_doctests-tests.rs"] mod tests;`, on fixture crates in the style of `doc_hidden-tests.rs` and `tidy-tests.rs`: the check fails on an untagged fence, a `rust` fence, an indented code block, and a `doc = include_str!`; it passes on `text`, `json`, and `toml` fences; an empty fixture fails.
- Gate:
  - In `.github/workflows/ci.yml`, remove the "Doctests" step (line 99) and the "Doctests (workshop)" step (line 213) with the comment above it.
  - In the Full suite line of `AGENTS.md`, delete ", then doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`".
- Checks:
  - The Project Survey commands for every crate this step touches, plus `cargo fmt --all --check`.
  - For every package this step edits: `cargo test --doc --all-features` reports zero doctests, `cargo nextest run --locked --all-features` passes, and `RUSTDOCFLAGS=-D warnings cargo doc --no-deps --all-features` passes.
  - `cargo nextest run --locked` passes for the Workshop crates, without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`.
  - `RUSTDOCFLAGS=-D warnings cargo doc -p harness --no-deps`, and `cargo check -p gateway --no-default-features`.
  - Before removing the gate, `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api` both report zero doctests.
  - `cargo test -p build-xtask` passes on the live workspace, and fails once a temporary `rust` fence is added to a doc comment in a library crate. Revert the probe.
  - `cargo hakari verify` passes.
  - The workspace docs command passes.

</step-2>

<step-3>

### Step 3: Rewrite every README and remove crates.io metadata

- Component: READMEs, manifests, and docs site
- Work item: 3 (READMEs) and 5 (crates.io metadata).
- READMEs, the 25 files:
  - The root `README.md` and `crates/README.md`.
  - The family indexes: `crates/promptforge-internal/README.md`, `crates/gateway/README.md`, and `crates/gateway/stt/README.md`.
  - The crate READMEs in `crates/gateway/{app,config,config-ui,local,protocol,routing,web-search}/`, `crates/gateway-api-discovery/`, `crates/harness-gateway-client/`, `crates/harness-internal/{capabilities,runner}/`, `crates/harness-web/`, `crates/promptforge-internal/{engine,lua,model-client,parser,types,vfs}/`, `crates/shared-loopback/`, and `crates/workshop/run-log/`.
- Each README becomes a few sentences stating the crate's or directory's responsibility in broad terms. Remove type, function, file, and crate names, dependency lists, crate lists, feature lists, commands, and numbers. Keep every file.
- The root `README.md` keeps its six images (`images/banner-02.png` through `images/banner-06.png`, and `images/promptforge-portrait.png`) and its CI and license badges.
- Remove everything tied to crates.io: the crates.io and docs.rs badges, and the `cargo install gateway` line in `crates/gateway/app/README.md`.
- Manifests:
  - In each crate's `[package]`, remove `description`, `readme`, `keywords`, `categories`, `documentation`, and `homepage`. Keep `description` on `crates/gateway/app` and `crates/workshop/desktop`, the two shipped binaries, because whether the Tauri bundler or cargo-dist reads it is unverified.
  - In the root `Cargo.toml` `[workspace.dependencies]`, remove `version` from every `path` entry (43 as of 2026-10-03, `workspace-hack` included), because Cargo reads it only when publishing.
  - Keep `publish = false` (all 49 crates have it), `license`, `repository`, and `[package.metadata.dist]`.
- Checks:
  - The Project Survey commands for the crates this step touches, plus `cargo fmt --all --check`.
  - `rg -i 'crates\.io|docs\.rs|cargo install' -g README.md` finds nothing, and every image the root `README.md` names exists.
  - `cargo build --locked -p gateway` and `cargo check --locked -p workshop` pass, and `Cargo.lock` is unchanged.
  - `cargo deny check` passes, and so does `dist plan` where cargo-dist is installed.
  - `cargo hakari verify` passes, and `cargo hakari manage-deps --dry-run` reports no change, because the root `workspace-hack` entry loses its `version`.
  - `rg '^(readme|keywords|categories|documentation|homepage)\s*=' -g Cargo.toml` finds nothing, and `^description` matches only the two binaries.

</step-3>

<step-4>

### Step 4: Generate the landing page and move the guides to promptforge-docs

- Component: READMEs, manifests, and docs site
- Work item: 4 (landing page) and 6 (guide move). Build the landing page first, then move the guides, because both edit the site build and the move builds on the generated page. Follow the `promptforge-docs` layout rule in Technical Design.
- Landing page, in `crates/build-xtask/src/site.rs`:
  - Each `RUSTDOC_SITES` entry gains a short, broad description.
  - A landing writer builds `target/site/index.html` from `RUSTDOC_SITES` and each staged book's `book.toml` `description`, so a new rustdoc site or book adds its row with no other edit. It replaces the copy of `guide/landing/index.html`. If `site.rs` would pass 500 lines, the writer lives in a private child module, `#[path = "site-landing.rs"] mod landing;`.
  - The page holds a title and one table, with no images and no other prose. Its columns are Documentation, Kind, and Covers. Its rows are the four API references (`promptforge`, `harness`, `harness-gateway-client`, `harness-web`) and the three guides (Prompt Language, Gateway, Workshop). Each Covers cell is a few broad words.
  - Update the module doc's build order.
- Each `guide/books/<book>/book.toml` gains a `description` under `[book]`.
- Remove `guide/landing/index.html` and `guide/landing/img/`, and drop the image and product-table styles from `guide/landing/style.css`.
- Guide move, in `c:\Users\Vinnie\cursor\promptforge-docs` (`git@github.com:cppalliance/promptforge-docs`, default branch `master`):
  - Copy `guide/src/language/`, `guide/src/gateway/`, `guide/src/workshop/`, and `guide/src/introduction.md` into `src/`, and `guide/CONTRIBUTING.md` to the root, as a plain copy with no history.
  - Commit there first and push `master` right away, before this step's promptforge commit, because the site workflow checks that repository out. Then `git rm` those paths from promptforge.
- What stays in promptforge: `guide/landing/`, `guide/chrome/`, `guide/books/*/book.toml`, `crates/build-user-guide`, and `crates/build-xtask/src/site.rs`. Point each `book.toml`'s `git-repository-url` at `https://github.com/cppalliance/promptforge-docs`.
- `crates/build-user-guide` (`src/main.rs`, `src/stage.rs`):
  - Read chapters from `$PROMPTFORGE_DOCS/src/`, and keep reading `book.toml` and `chrome/` from `guide/`.
  - Read the variable once in `main` and pass the path down, so tests need no `std::env::set_var`, which is unsafe in Rust 2024.
  - Write the single-file exports `promptforge-<book>-guide.md` into the site output, where `cargo xtask site` places them in `target/site/`, instead of into `guide/`. Remove the committed `guide/promptforge-*-guide.md` files.
  - Update the module docs that name `guide/src/`.
- `cargo xtask site` reads `PROMPTFORGE_DOCS` once in `main`, passes the path down and through to the stage run, and fails with a message naming the variable when it is unset or names no such directory.
- `.github/workflows/site.yml`:
  - Check out `cppalliance/promptforge-docs` beside promptforge with `actions/checkout`, `repository:`, and `path:`, and set `PROMPTFORGE_DOCS` for both build steps.
  - Drop the `guide/**` pull-request path trigger.
  - Add a nightly `schedule` and keep `workflow_dispatch`. The deploy job's `github.event_name != 'pull_request'` condition already covers scheduled runs.
- `AGENTS.md`: delete "; user guide: `cargo xtask site --books-only`" from the Docs line.
- Tests:
  - In `crates/build-xtask/src/site-tests.rs`: every `RUSTDOC_SITES` entry and every staged book gets exactly one row, the page has no `<img>`, and `cargo xtask site` fails with `PROMPTFORGE_DOCS` in the message when the docs root is unset or missing.
  - In `crates/build-user-guide/src/stage-tests.rs`: staging reads chapters from the docs root it is given, and an unset or missing root fails with `PROMPTFORGE_DOCS` in the message. The `fake_guide()` fixture in `src/main.rs` moves to the split layout.
- This step pushes only `promptforge-docs`. Pushing promptforge, running the site workflow, and checking CI happen in the run's last step (Step 9), under the push rules in Technical Design.
- Checks:
  - The Project Survey commands for `build-xtask` and `build-user-guide`, plus `cargo fmt --all --check`.
  - `cargo nextest run --locked -p build-user-guide -p build-xtask` and `cargo test -p build-xtask`.
  - With `PROMPTFORGE_DOCS` set to `c:\Users\Vinnie\cursor\promptforge-docs`: `cargo xtask site --books-only` passes its link check, and `cargo xtask site` builds, writes the landing page with all seven rows, passes its link check, and lands the exports in `target/site/`.
  - `git -C c:\Users\Vinnie\cursor\promptforge-docs status -sb` shows `master` with no unpushed commit.

</step-4>

<step-5>

### Step 5: Remove stale enforcement and make every crate inherit one strict lint table

- Component: Structural enforcement
- Work item: 7 (stale enforcement) and 8 (one strict lint table). Do the parts in order: stale config, then the lint table, then the strict lints, because the strict lints need every crate on the workspace table.
- Stale enforcement code and config:
  - Delete `crates/harness-internal/runner/clippy.toml` and `crates/harness-internal/capabilities/clippy.toml`, which only restate the root `clippy.toml` now that their tokio bans are gone.
  - In `.cursor/rules/workshop-spa.mdc` line 13, point the reference to the deleted `crates/workshop/ui/src/services/panel-registry.ts` at `crates/workshop/platform/panel-registry.ts`, without line numbers.
  - Replace or drop the reason at `deny.toml` line 13, which cites `paste` through a candle/gemm stack that isn't in `Cargo.lock`.
  - Remove the unused `clippy` component from the `check-workshop-linux` toolchain step in `.github/workflows/ci.yml` (lines 250 to 252).
  - Fix the module doc of `crates/build-xtask/src/harness_bans.rs` (lines 12 and 13), which calls the live check vacuously true.
- One lint table:
  - Root `Cargo.toml` `[workspace.lints.rust]`: lower `unsafe_code` from `forbid` to `deny`, because a `forbid` can't be relaxed by a module-level `expect`. Keep every other entry.
  - Replace the hand-copied `[lints.rust]`, `[lints.rustdoc]`, and `[lints.clippy]` tables in `crates/gateway/app`, `crates/gateway/stt/whisper-ffi`, `crates/gateway-api-discovery`, and `crates/workshop/desktop` with `[lints] workspace = true`. Their unsafe code stays behind the module-level `#[expect(unsafe_code, reason = ...)]` they already use. Fix the `pedantic` findings that now deny in those four crates, whose copies had it at `warn`.
  - `crates/build-xtask/src/tidy.rs`, both checks called from `all_violations`, each failing when it scanned nothing:
    - `lint_inheritance_violations` binds every crate from `crate::product::workspace_crates`, unread ones included, instead of `participating_crates`.
    - A new `unsafe_allowlist_violations` fails when any `.rs` file outside the four crates above expects or allows `unsafe_code`, directly or through `cfg_attr`. The fixture strings in `crates/build-xtask`'s own tests must not trip it.
  - Tests in `crates/build-xtask/src/tidy-tests.rs`, on fixture crates: for each check, a passing fixture, a failing fixture (a crate without `[lints] workspace = true`; `#[expect(unsafe_code)]` outside the four crates), and an empty fixture that fails.
  - In `.github/workflows/ci.yml`, remove the "Check safe STT crates without unsafe code" and "Check native STT FFI lint policy" steps, which these checks replace.
- Strict clippy lints:
  - Root `[workspace.lints.clippy]` adds `undocumented_unsafe_blocks`, `missing_safety_doc`, `allow_attributes`, `allow_attributes_without_reason`, `exit`, `disallowed_methods`, `disallowed_types`, and `disallowed_macros`, all at `deny`. `pedantic` stays at `deny`.
  - Each `#[allow]` becomes `#[expect(..., reason = ...)]`, or `#[cfg_attr(..., expect(...))]` where the lint fires on one platform only. As of 2026-10-03 there are four: `crates/workshop/server/src/app/test_helpers.rs:11`, `crates/workshop/support/src/fixtures.rs:9`, and `crates/gateway/stt/api/src/test_fixtures/hour.rs:238` and `:303`. The `#[allow]` text in `crates/build-xtask/src/*-tests.rs` is fixture data and stays.
  - Remove the two `process::exit` calls outside `main` (`crates/build-user-guide/src/main.rs:236` and `crates/workshop/desktop/src/main.rs:220`), or keep them under `#[expect(clippy::exit, reason = ...)]`.
  - Move the SAFETY comment inside the block at `crates/gateway/app/src/main.rs:71` to the line before the block.
- Checks:
  - The Project Survey commands for the crates this step touches, plus `cargo fmt --all --check`.
  - `cargo test -p build-xtask`, both clippy commands, and `cargo deny check`.
  - Clippy on `harness-runner` and `harness-capabilities` (all targets, all features) passes under the root `clippy.toml`.
  - `cargo check -p gateway --no-default-features`, and the docs commands.
  - `cargo nextest run --locked` for the four crates whose hand-copied lint tables this step replaces.

</step-5>

<step-6>

### Step 6: Ban process-global installers, tighten the gate, and shrink AGENTS.md

- Component: Structural enforcement
- Work item: 9 (installer bans), 10 (serde_json `preserve_order`), 11 (gate settings), and 12 (`AGENTS.md`). Edit `AGENTS.md` last, because it deletes the prose that the lint table, the strict lints, the bans, and the gate settings replace.
- Process-global installers: the root `clippy.toml` keeps its two test allowances and gains `disallowed-methods` entries:
  - `std::panic::set_hook`, `log::set_logger`, `log::set_boxed_logger`, and `tracing::subscriber::set_global_default`.
  - `tracing_subscriber::util::SubscriberInitExt::init` and `try_init`.
  - `rustls::crypto::CryptoProvider::install_default`.
  - Each `reason` reads "use X instead, because Y". Add `replacement` only where the call shape matches, and drop any entry whose crate isn't in `Cargo.lock`.
  - The three subscriber `init()` calls in `crates/gateway/app/src/main.rs` get `#[expect(clippy::disallowed_methods, reason = ...)]`, and so does any other installer clippy flags in a binary entry point; only binary entry points may install process-global state. mlua's `lua.set_hook` in `crates/promptforge-internal/lua` is a different method and is unaffected.
- serde_json `preserve_order`: `deny.toml` gains a `[[bans.features]]` entry with `crate = "serde_json"` and `deny = ["preserve_order"]`.
- Gate settings:
  - `.cargo/config.toml` gains `[env] CLIPPY_DISABLE_DOCS_LINKS = "1"`. Cargo applies `[env]` to the compiler processes it runs, which should cover `clippy-driver`, but that is unverified, so confirm that clippy output no longer prints the docs-link line.
  - Replace `-- -D warnings` with `CARGO_BUILD_WARNINGS=deny` in `.github/workflows/ci.yml` (the two clippy steps, lines 67 and 192) and `.githooks/pre-push` (line 9).
    - The gate never sets `RUSTFLAGS`. On Windows that would replace the `+crt-static` setting in `.cargo/config.toml`, and anywhere it changes compiler flags and throws away the build cache.
    - The setting stays out of `.cargo/config.toml`, so a new stable lint fails only the gate, not every local build.
    - Keep `RUSTDOCFLAGS="-D warnings"` unless `CARGO_BUILD_WARNINGS` is confirmed to cover rustdoc.
  - In the `supply-chain` job of `ci.yml`, add `cargo-hakari` to the existing `taiki-e/install-action` step's `tool:` list, and add a `cargo hakari verify` step.
  - Run `git config core.hooksPath .githooks` in this clone.
- `AGENTS.md`, preferring deletion over addition and finding each edit by its quoted text:
  - Engineering bullets:
    - Replace "Add a structural check only with explicit user approval for a stable product or security boundary that has no ordinary equivalent." with "Gate on yes/no properties, never on a score threshold that ordinary work edits."
    - Delete the bullet that begins "A plan cannot introduce a source parser".
    - Delete "Structural tests that an approved plan identifies as unsupported may be removed without replacement by another structural proxy."
    - Delete "Library and serve paths return failures instead of exiting the process or installing process-global state."
    - Delete "Every unsafe block documents its safety invariants immediately before the block." Delete the matching clauses in the crate files: ", and every unsafe block has a `// SAFETY:` comment on the immediately preceding line" in `crates/workshop/desktop/AGENTS.md`, and "; every `unsafe` block has a `// SAFETY:` comment on the immediately preceding line naming the lifetime, null, and ownership invariants the call relies on" in `crates/gateway/stt/whisper-ffi/AGENTS.md`.
    - Delete ", and serde_json `preserve_order` is never enabled".
  - Linter line:
    - Drop `-- -D warnings` from both clippy commands, and state once that the gate runs them with `CARGO_BUILD_WARNINGS=deny`.
    - Replace "Clippy is a superset of `cargo check` and shares no artifacts with it, so never run" with "Clippy reports everything `cargo check` does, so never run".
  - SPA and CSS bullets:
    - Replace the CSS bullet with "No raw color, size, or spacing values in Workshop UI component CSS; use the custom properties defined in `@workshop/look` and `ui/src/tokens/component.css`."
    - In the `localStorage` bullet, delete "No `localStorage`. The SPA never reads or writes browser storage;", which `crates/workshop/ui/test/no-local-storage.mjs` enforces, and keep the rest, from "every persisted value goes through", capitalized.
  - Keep the upstream-issue-URL rule and the flat-directory paragraph.
- Checks:
  - The Project Survey commands for the crates this step touches, plus `cargo fmt --all --check`.
  - Both clippy commands and `cargo deny check`.
  - A temporary `std::panic::set_hook` call in a library crate fails clippy, and temporarily enabling `serde_json/preserve_order` in a manifest fails `cargo deny check`. Revert both probes.
  - With `CARGO_BUILD_WARNINGS=deny`, a temporary unused variable fails clippy, which passes again once it is removed.
  - Clippy output no longer prints the docs-link line.
  - `cargo hakari verify` passes locally; if it doesn't, run `cargo hakari generate` and commit the result in this step.
  - `git config core.hooksPath` prints `.githooks`, and `.githooks/pre-push` runs clean.
  - `node --test test/docs-claims.mjs` from `crates/workshop/ui` passes, and none of the deleted sentences remains in any `AGENTS.md`.

</step-6>

<step-7>

### Step 7: Split every file over 500 lines

- Component: Compiler-enforced 500-line limit
- Work item: 13, splits. This step wires no crate to the compile-time check.
- Scope: split every `.rs` file over 500 lines in the workspace, including any that grew past the limit after 2026-10-03. List them from the promptforge root with `Get-ChildItem crates -Recurse -File -Filter *.rs | Where-Object { $_.FullName -notmatch '\\(target|node_modules)\\' -and @(Get-Content -LiteralPath $_.FullName).Count -gt 500 } | Select-Object -ExpandProperty FullName`. As of 2026-10-03 there are 66, all in the groups below.
- Divide the crates among parallel sub-agents, one crate group each, on disjoint files, using the seven groups below. A file the scan finds outside the lists goes to the group that owns its crate, or to the coding agent when no group does.
- Split rules:
  - Split each file under the split rule in Technical Design. Pick the `#[path = "<stem>-<topic>.rs"]` or `<stem>/<topic>.rs` form by the flat-directory rule in `AGENTS.md`.
  - Never widen anything to `pub(crate)` or `pub` to make a split compile. If no cohesive group splits out without widening, stop and report why.
- Groups and files, with line counts as of 2026-10-03:
  - Gateway app source, 16 files in `crates/gateway/app/src/`: `runner.rs` (1,592), `commands.rs` (1,469), `dialect.rs` (1,153), `error.rs` (1,095), `tray/logic.rs` (908), `boot.rs` (869), `main.rs` (816), `admin/walled/config_apply-tests.rs` (701), `tray/windows.rs` (660), `boot-speech-tests.rs` (660), `tray/macos.rs` (623), `tray/linux.rs` (601), `auth.rs` (578), `routing.rs` (572), `test_support.rs` (534), and `lib.rs` (530). The tray files compile on one platform each. Check the Linux and macOS splits with `cargo check --target` where that builds, and otherwise rely on CI's Linux and macOS jobs.
  - Gateway app tests, 6 files in `crates/gateway/app/tests/it/`: `speech.rs` (1,805), `boot.rs` (1,317), `chat.rs` (1,073), `support.rs` (655), `cuda.rs` (575), and `realtime_stt.rs` (569).
  - `gateway-local`, 13 files in `crates/gateway/local/src/`: `runtime.rs` (2,127), `artifacts/tests.rs` (1,918), `server-tests.rs` (1,537), `cache.rs` (1,169), `artifacts.rs` (873), `server.rs` (595), `dialect.rs` (592), `sidecar.rs` (571), `artifacts/download.rs` (553), `error.rs` (552), `server-support.rs` (539), `artifacts/confine.rs` (519), and `upstream.rs` (516).
  - `gateway-config` and `gateway-logging`:
    - `crates/gateway/config/src/`: `config/tests/validation.rs` (2,066), `config/accessors.rs` (1,952), `config/validate.rs` (888), `config/companion.rs` (676), `config.rs` (626), and `config/tests.rs` (530).
    - `crates/gateway/logging/src/`: `worker.rs` (1,661), `queue.rs` (1,569), `writer.rs` (839), and `redact.rs` (686).
  - The smaller gateway crates, `gateway-protocol`, `gateway-routing`, `gateway-cloud-providers`, and `gateway-web-search`:
    - `crates/gateway/protocol/src/`: `upstream.rs` (1,772) and `wire.rs` (1,336).
    - `crates/gateway/routing/src/queue.rs` (507).
    - `crates/gateway/cloud-providers/src/`: `sheet.rs` (715) and `lib.rs` (574).
    - `crates/gateway/web-search/src/`: `service.rs` (616) and `process.rs` (525).
  - The STT crates, `gateway-stt` and `gateway-stt-engine`:
    - `crates/gateway/stt/api/`: `src/audio.rs` (531), `src/take/window.rs` (501), `src/test_fixtures.rs` (613), `tests/it/realtime_session.rs` (953), `tests/it/realtime_fixtures.rs` (673), and `tests/it/realtime_forced_windows.rs` (615).
    - `crates/gateway/stt/engine/src/worker.rs` (520).
  - The remaining crates, `gateway-api-discovery`, `build-llama-cuda`, `build-workshop`, and `workshop`:
    - `crates/gateway-api-discovery/src/`: `health.rs` (1,119), `validated.rs` (771), `stale.rs` (638), and `lock.rs` (501).
    - `crates/build-llama-cuda/src/bundle.rs` (893).
    - `crates/build-workshop/src/main.rs` (1,329).
    - `crates/workshop/desktop/src/gateway/tests/boot.rs` (514).
- Checks:
  - The Project Survey commands for the crates this step touches, plus `cargo fmt --all --check`.
  - The scan above lists no file.
  - `cargo nextest run --locked --all-features` and `cargo build` pass for `gateway`, `gateway-local`, `gateway-config`, `gateway-logging`, `gateway-protocol`, `gateway-routing`, `gateway-cloud-providers`, `gateway-web-search`, `gateway-stt`, `gateway-stt-engine`, `gateway-api-discovery`, `build-llama-cuda`, and `build-workshop`, and for the package of any other file the scan found.
  - `cargo nextest run --locked -p workshop` and `cargo build --locked -p workshop`.
  - `cargo check -p gateway --no-default-features`.
  - Both clippy commands.
  - `cargo test -p build-xtask`, because the tidy ceiling still binds the 23 crates with the `## Invariants` marker.

</step-7>

<step-8>

### Step 8: Add the build-ceiling crate, wire every crate, and retire the tidy ceiling

- Component: Compiler-enforced 500-line limit
- Work item: 13, the compile-time check, its wiring into every crate with the wiring tidy check, the new-crate template, and the removal of what the check replaces.
- Before wiring, confirm that no `.rs` file in the workspace is over 500 lines, because a wired crate with one fails its build. From the promptforge root, `Get-ChildItem crates -Recurse -File -Filter *.rs | Where-Object { $_.FullName -notmatch '\\(target|node_modules)\\' -and @(Get-Content -LiteralPath $_.FullName).Count -gt 500 } | Select-Object -ExpandProperty FullName` lists nothing. Split any file it lists, and any file this step pushes past 500 lines, under the split rule in Technical Design.
- The `build-ceiling` crate:
  - New crate `crates/build-ceiling` (package `build-ceiling`): no dependencies of any kind, not even `workspace-hack`; `publish = false`; `[lints] workspace = true`.
  - Add `build-ceiling = { path = "crates/build-ceiling" }` to the root `[workspace.dependencies]`.
  - Add `build-ceiling` to `[final-excludes] workspace-members` in `.config/hakari.toml`, beside `promptforge-vfs`, so hakari never adds `workspace-hack` to it.
- `src/lib.rs` exposes `pub fn check() -> Result<(), Violations>` and `pub struct Violations`, as the `build_ceiling::check()` interface in Technical Design sets out:
  - `check()` reads the calling crate's root from `CARGO_MANIFEST_DIR` and passes it to the scan, which takes the root as a parameter, so tests need no environment variable.
  - The scan counts every physical line with `str::lines().count()`, so blank, comment, and test lines all count, in every `.rs` file under `src/`, `tests/`, `benches/`, and `examples/`, plus `build.rs`.
  - It emits `cargo::rerun-if-changed` for those paths, directories included, so an added file reruns the check.
  - `Violations` lists every file over 500 lines and every file the scan can't read, which it reports instead of skipping. Its `Display` and `Debug` both render the ceiling failure message from Technical Design: the path, the line count, the limit, the split rule as `help:` lines, and "If no cohesive group splits out without widening visibility, stop and say why."
  - Nothing in the crate calls `process::exit` or panics.
- `build-ceiling`'s own `build.rs` declares `#[path = "src/lib.rs"] mod build_ceiling;`, with `#[expect(unreachable_pub, reason = ...)]` on that declaration if the lint fires, and its `main` is `fn main() -> Result<(), build_ceiling::Violations> { build_ceiling::check() }`, instead of depending on itself.
- Unit tests, on std-only fixtures with no dev-dependencies:
  - Every physical line counts: blank lines, comments, CRLF endings, and a missing final newline.
  - A 500-line file passes and a 501-line file fails.
  - Both the `Display` and the `Debug` text have the path, the count, the limit, the split rule, and the stop sentence.
  - An unreadable file, such as one with invalid UTF-8, is reported, not skipped.
- Wiring, under the ceiling wiring rule in Technical Design. Every workspace crate except `build-ceiling` and `workspace-hack` gets a package-root `build.rs` that calls `build_ceiling::check()`:
  - Dependency form, in every crate except the six `#[path]` crates below: `build-ceiling.workspace = true` under `[build-dependencies]`, and a new `build.rs` whose `main` is `fn main() -> Result<(), build_ceiling::Violations> { build_ceiling::check() }`. This includes `gateway-local`, `gateway-config`, `gateway-logging`, `gateway-protocol`, `gateway-routing`, `gateway-cloud-providers`, `gateway-web-search`, `gateway-stt`, `gateway-stt-engine`, `gateway-api-discovery`, `build-llama-cuda`, and `build-workshop`.
  - The four existing build scripts take the same `[build-dependencies]` entry and call `build_ceiling::check()` first in `main`:
    - `crates/gateway/app/build.rs` (package `gateway`), whose `main` returns `anyhow::Result<()>`: call `build_ceiling::check()?` first, before the icon and manifest work.
    - `crates/workshop/desktop/build.rs` (package `workshop`), whose `main` returns `Result<(), Box<dyn Error>>`: call `build_ceiling::check()?` first, before the sidecar refresh and the Tauri build.
    - `crates/gateway/config-ui` and `crates/workshop/server`, whose `main` returns an `ExitCode`: call `build_ceiling::check()` first, and on an error print it with `eprintln!("{error}")` and return `ExitCode::FAILURE`.
  - `promptforge-parser` gets a new package-root `build.rs`. Its `src/build.rs` is an ordinary module, not a build script, and stays as it is.
  - `#[path]` form, with no manifest entry, in `harness`, `harness-gateway-client`, `harness-web`, `harness-runner`, `harness-capabilities`, and `promptforge-vfs`: the `build.rs` declares `#[path = "<relative path>/build-ceiling/src/lib.rs"] mod build_ceiling;` and calls `build_ceiling::check()`. Put `#[expect(unreachable_pub, reason = ...)]` on the `mod` declaration if that lint fires. These six take no manifest entry because `crates/build-xtask/src/product.rs` bars the Harness crates from every `build-*` crate in every dependency table, and `promptforge-vfs`'s `the_manifest_declares_no_dependencies` test refuses any build-dependency. Both rules stay unchanged.
  - `workspace-hack` gets no build script.
- Ceiling wiring tidy check: `crates/build-xtask/src/tidy.rs` gains `ceiling_wiring_violations`, called from `all_violations`:
  - Every workspace crate's package-root `build.rs` calls `build_ceiling::check()`, and either names `build-ceiling` under `[build-dependencies]` or includes `build-ceiling/src/lib.rs` with `#[path]`. A `src/build.rs` module never counts as a build script.
  - The two exceptions are `build-ceiling`, whose `build.rs` includes its own source, and `workspace-hack`, whose `.rs` files the check counts against the same 500-line limit.
  - It fails when it scanned nothing.
- `cargo xtask new-crate` (`crates/build-xtask/src/new_crate.rs`) scaffolds only Workshop crates, so it writes the dependency-form `build.rs` and the `[build-dependencies]` entry into every crate it scaffolds, and its tests assert both.
- Tests in `crates/build-xtask/src/tidy-tests.rs`, split under the split rule if it would pass 500 lines: a dependency-wired fixture and a `#[path]`-wired fixture pass; a `build.rs` without the call fails; a crate with no `build.rs` fails; a `workspace-hack` file over 500 lines fails; an empty fixture fails.
- A `build.rs` failure also fails release and installer builds. CI's clippy and test jobs build every crate first, so they catch a violation before a release does.
- Retire the tidy ceiling once every crate is wired, removing what the compile-time check replaces:
  - In `crates/build-xtask/src/tidy.rs`: `MAX_FILE_LINES`, `file_ceiling_violations` and its call in `all_violations`, and `participating_crates`, which has no other caller now that `lint_inheritance_violations` binds every crate from `crate::product::workspace_crates`. Update the module doc's sentence about the file ceiling. Remove the ceiling tests in `crates/build-xtask/src/tidy-tests.rs`.
  - The "Every file in this crate stays under 500 lines; split first, then edit." bullet in the `## Invariants` docs of 23 crates: `crates/build-xtask/src/main.rs`; the `lib.rs` of `harness-web`, `harness-gateway-client`, `harness-runner`, and `harness-capabilities`; `promptforge-engine`, `promptforge-lua`, `promptforge-model-client`, `promptforge-parser`, `promptforge-types`, and `promptforge-vfs`; and `workshop-agents`, `workshop-gateway`, `workshop-menu`, `workshop-protocol`, `workshop-registry`, `workshop-run-log`, `workshop-server`, `workshop-server-api`, `workshop-status`, `workshop-support`, `workshop-user-state`, and `workshop-workspace`. Remove the same bullet from the `lib_rs` template in `crates/build-xtask/src/new_crate.rs`.
  - In `.cursor/rules/workshop-architecture.mdc`, the "File ceiling" section, and ", 500-line file ceiling" from the `description` line.
  - In the module doc of `crates/workshop/server/src/agents/socket_frames.rs`, the clause "so each stays under the 500-line ceiling".
  - In the structural-checks line of `AGENTS.md`, ", and the 500-line file ceiling", restoring "and" before the last remaining item.
- Checks:
  - The Project Survey commands for the crates this step touches, plus `cargo fmt --all --check`.
  - `cargo nextest run --locked -p build-ceiling` and `cargo build -p build-ceiling`.
  - `cargo test -p build-xtask` and `cargo hakari verify`.
  - `cargo build --locked -p gateway`, `cargo build --locked -p workshop`, and `cargo check -p gateway --no-default-features`.
  - Both clippy commands and the full nextest suite.
  - A temporary 501-line file in `crates/gateway/app/src/` fails `cargo build -p gateway` with the ceiling failure message, and the build passes again once it is removed.
  - The scan above lists no file.
  - `rg -F 'Every file in this crate stays under 500 lines'` finds nothing, and `node --test test/docs-claims.mjs` from `crates/workshop/ui` passes.
  - Measure and report, without gating on it, how long one rerun of each of the four existing build scripts takes (`gateway`, `gateway-config-ui`, `workshop-server`, and `workshop`). Those scripts used to rerun only on their own inputs; the ceiling's `rerun-if-changed` lines now rerun them, esbuild and the Tauri build included, on every source edit in their crates.

</step-8>

<step-9>

### Step 9: Narrow visibility, verify the whole plan, push, and watch CI and the site

- Component: Compiler-enforced 500-line limit
- Work item: 13, visibility, then the final verification of all 13 items, the promptforge push, and the CI and site workflow runs.
- Divide the visibility work among parallel sub-agents by family (Engine; Harness; gateway and shared; Workshop and build crates), each on disjoint files. Once their changes are merged, run the checks, then push.
- Visibility, under the visibility rule in Technical Design:
  - Give every `pub(crate)` and `pub(super)` item the narrowest visibility that compiles across all targets and features: private where only its own module uses it, `pub(super)` where only the parent does.
  - Method: in each crate, narrow every such item to private, compile across all targets and features, then restore only the items the compiler reports as inaccessible, each at the narrowest level that compiles. Treat a `private_interfaces` or `private_bounds` warning as a report too.
  - Compile every feature shape CI builds: `--all-features` for most crates; default features and `--features headless` for `workshop-server`; default features for `workshop` and `workshop-server-api`; and `--no-default-features` for `gateway`.
  - Leave unchanged any item in code that no local build compiles, such as the macOS and Linux tray files and other `cfg` branches for another platform, unless `cargo check --target <triple>` builds it on this machine. List the skipped files in the ledger.
  - The pass spans about 3,300 `pub(crate)` and `pub(super)` uses in about 490 files.
- Families:
  - Engine: `promptforge` and every crate under `crates/promptforge-internal/`. The facade surface must not change. If a change does reach `crates/promptforge/public-api.txt`, regenerate it with `cargo +nightly-2026-09-05 xtask api --bless`, confirm with `--check`, and review the diff.
  - Harness: `harness`, `harness-gateway-client`, `harness-web`, and every crate under `crates/harness-internal/`.
  - Gateway and shared: every crate under `crates/gateway/`, plus `gateway-api-types`, `gateway-api-discovery`, `shared-loopback`, and `shared-error-source`.
  - Workshop and build crates: every crate under `crates/workshop/`, and every `build-*` crate, `build-ceiling` and `build-xtask` included.
- Checks, run by each sub-agent for its family and by the coding agent after the merge:
  - `cargo nextest run --locked --all-features` for each family's packages, without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`.
  - Both clippy commands and `cargo test -p build-xtask`.
  - Engine: the facade docs and `cargo +nightly-2026-09-05 xtask api --check`.
  - Harness: `RUSTDOCFLAGS=-D warnings cargo doc -p harness --no-deps`.
  - Gateway and shared: `cargo check -p gateway --no-default-features`.
  - Workshop and build crates: `cargo build --locked -p workshop`.
- Final verification, after the merge:
  - CI's two private-item doc builds pass with `RUSTDOCFLAGS=-D warnings`: `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items` and `cargo doc --locked --no-deps -p workshop-server --document-private-items`.
  - Every Project Survey command passes, plus `cargo fmt --all --check`, with clippy run under `CARGO_BUILD_WARNINGS=deny`. The exception is the two doctest commands, which the gate no longer runs: run `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api` once more, and both report zero doctests.
  - `rg 'doc = include_str!' crates` matches only the fixture strings in `crates/build-xtask/src/*-tests.rs`.
  - No `.rs` file in the workspace exceeds 500 lines.
  - A deliberately oversized scratch file fails a crate's `cargo build` with the ceiling failure message, and the build passes once it is removed. Try one dependency-wired crate and one `#[path]`-wired crate.
  - `cargo test -p build-xtask` fails on each probe: `unsafe_code` expected outside the four allowed crates (`crates/gateway/app`, `crates/gateway/stt/whisper-ffi`, `crates/gateway-api-discovery`, and `crates/workshop/desktop`), a crate without `[lints] workspace = true`, a crate whose `build.rs` doesn't call `build_ceiling::check()`, a `workspace-hack` file over 500 lines, and a compiled code block in a doc comment.
  - `cargo deny check` fails with serde_json's `preserve_order` turned on, and clippy fails on a `std::panic::set_hook` call in a library crate.
  - Revert every probe.
  - With `PROMPTFORGE_DOCS` set to `c:\Users\Vinnie\cursor\promptforge-docs`, `cargo xtask site` builds, writes the seven-row landing table, and passes its link check.
  - `tools/cicerone.md` and `tools/cicerone/` no longer exist in promptforge.
- Push and watch, once every check above passes. The session, not a sub-agent, does this after Step 9's commit message is final and the run's `Close plan` commit is made, so nothing provisional is pushed. A fix after the push lands as a new commit, never an amend.
  - Push promptforge from `c:\Users\Vinnie\cursor\promptforge` with `git push`.
  - Start the site workflow with `gh workflow run site.yml`.
  - Find the CI run for the pushed commit (`gh run list --workflow ci.yml --commit <sha>`) and the new site run (`gh run list --workflow site.yml --limit 1`), and watch each with `gh run watch <run-id> --exit-status` until both succeed.
  - Fix any failure in this step: read it with `gh run view <run-id> --log-failed`, fix the cause, rerun the checks for what the fix touches, make a new commit, push, start the site workflow again, and watch both new runs until they succeed.
  - Confirm that the deployed site's guide pages come from `promptforge-docs`: the site run's log shows the checkout of `cppalliance/promptforge-docs`, and a guide chapter on the deployed site (at the Pages URL that `gh api repos/{owner}/{repo}/pages --jq .html_url` prints) links to `https://github.com/cppalliance/promptforge-docs` and matches its source under `src/<book>/` in that repository.

</step-9>

</execution-plan>
