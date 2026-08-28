---
name: Digest Marker, Child Priority, and Switch-Failure Diagnosis
overview: "Gateway work in three commits plus one gating spike: (0) capture the real profile-switch failure error before fixing anything, (1) a verified-digest marker so profile switches stop re-hashing multi-GB GGUFs on every cache hit, (2) llama-server children at below-normal priority on Windows, (3) Gemma-4 dialect recognition only if step 0 proves dialect resolution is the failure."
todos:
  - id: diagnose-switch
    content: "Step 0 (spike): reproduce the profile switch and capture the SSE terminal error chain"
    status: completed
  - id: digest-marker
    content: "Commit 1: verified-digest marker in local/artifacts (verify_blob + markers, wire into ensure_blob/ensure_model, 6 tests)"
    status: completed
  - id: child-priority
    content: "Commit 2: below-normal priority for llama-server children on Windows (production_command + creation_flags, cfg(windows) test via windows-sys dev-dep)"
    status: completed
  - id: gemma-dialect
    content: "Commit 3 (CONDITIONAL on step 0): Gemma-4 dialect recognition in local/dialect.rs - only if diagnosis proves NoMatch"
    status: completed
  - id: final-verify
    content: "Final verification: full workspace suite + fmt/clippy gates"
    status: completed
isProject: false
---

# Digest Marker, Child Priority, and Switch-Failure Diagnosis

**Task size (vibe-rulebook): Bounded** - one spike plus two committed fixes, with a third commit conditional on the spike. Governing: `tools-public/rulebooks/vibe-rulebook.md` (process), `tools-public/rulebooks/rust-rulebook.md` (all code), `promptforge/AGENTS.md` (house rules; an earlier glob confirmed no nested AGENTS.md exists under `promptforge-gateway`, so the root file is the whole manifest for every step).

**Review history:** this plan was revised after a defect pass found (a) the Gemma-4 dialect commit rested on an unproven root cause, and (b) the priority commit's rationale overstated its effect. Both are corrected below.

## Run machinery (vibe-rulebook)

- **Scratch dir:** `cabinet/_scratch/vibe-gateway-switch-fixes/` holds `vibe-ledger.md` (append-only, one line per step: step, commit hash, Verify status, solo decisions with falsifiers) and `vibe-review.md` (open findings, carried forward verbatim between reviews). Main writes the ledger itself.
- **Worktree gate:** if the promptforge worktree is dirty at run start, stop and tell the user to commit or stash first. The tool never pushes.
- **Spike (Step 0):** per the rulebook's Spike path - no kept code, no review: find the answer, report it, delete the artifacts. Its outcome routes the run: `DialectResolution`/`NoMatch` unlocks Commit 3; anything else cancels Commit 3 and the plan is updated with what the spike actually found before any fix is built.
- **Per-commit checklist (Commits 1-3):** create the step checklist; dispatch the **Coder** subagent (role, this plan's path, step id, `<rule-book>` block name, `promptforge/AGENTS.md`, `tools-public/rulebooks/rust-rulebook.md`); commit; dispatch **Review-and-Fix** (adds `<code-review>`); amend if it dirtied the tree; run **Verify** when scheduled.
- **Verify schedule:** when review-and-fix dirtied the tree, and on the final commit. Final Verify runs `cargo test --locked --workspace --all-features`; earlier Verifies run the step's focused command (`cargo test --locked -p promptforge-gateway` with the step's filter). An unfixed Critical finding blocks the next commit; three red Verify rounds stop the run.
- **Commit messages:** first line <= 60 chars; body 100-400 tokens with an overview of the high-level changes; zero to 3 bullets for non-obvious notes or plan deviations with what forced them; no step numbers.
- **Solo decisions** (rule 2): reversible calls are made and recorded in the ledger with falsifiers; the spike's outcome routing is the only planned branch point.

## Confirmed facts (from the live diagnosis session)

- `gemma-4-31B-it-UD-Q4_K_XL.gguf` (18.8 GB) and `Qwen3.8-27B-Q8_0.gguf` (29 GB) are fully downloaded in the cache; `Qwen3.5-9B-Q4_K_M.gguf` exists only as a 2 GB `.part` (incomplete download).
- Gemma-4 loads in the pinned Vulkan llama-server (b10082) in ~24 s with the gateway's exact launch flags; `/props` reports `chat_template_caps.supports_tool_calls: true`.
- The gateway's dialect probe reads `has_tool_call_capability` from `/v1/models` ([local/dialect.rs:290](promptforge/crates/promptforge-gateway/src/local/dialect.rs:290)), and per that code's own comment a `--jinja` server with a tool-capable template reports it - so the existing code likely already resolves Gemma-4 to `"openai"`.
- The actual switch-failure error was never captured; the UI shows only the generic label (`chat_ws.rs` pushes the gateway's detailed message alongside it).

## Step 0 (spike): capture the real failure

Reproduce the switch with the gateway in the foreground and read the terminal error event, which carries the full source chain via `error_chain`:

- Run `promptforge-gateway serve` against `~/.promptforge/gateway.toml` with tracing at `debug` for `promptforge_gateway`.
- POST `/admin/switch-profile` with the failing profile name and record the SSE stream's terminal `error` event message.
- Also record which profile was being switched to when the failure occurred.

Outcome routing: if the chain ends in `DialectResolution`/`NoMatch`, proceed to Commit 3 as written. If it ends anywhere else (download, readiness, routing, store), fix THAT instead and Commit 3 is cancelled. Spike artifacts are deleted after the answer is recorded.

## Commit 1: verified-digest marker for cached models

New submodule `crates/promptforge-gateway/src/local/artifacts/verified.rs`:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub(super) enum VerifyOutcome {
    MarkerHit,
    Hashed,
}

/// Verifies `path` matches `expected` SHA-256 digest, consulting `marker` for cached verification.
///
/// # Errors
/// Returns [`LocalError`] if hashing fails, file IO fails, or digest mismatches.
pub(super) fn verify_blob(
    cache_root: &Path,
    path: &Path,
    expected: &str,
    marker: &Path,
) -> Result<VerifyOutcome, LocalError>
```

- Marker format: plain lines via `write_synced` - line 1 the verified SHA-256 hex, line 2 file size, line 3 mtime as `<secs>.<nanos>` from `UNIX_EPOCH`.
- Validation: `validate_cache_path` on both paths; marker exists AND pin matches AND size+mtime match -> `MarkerHit`, zero disk reads. Missing/stale/corrupt -> `file_digest`; on match write the marker and return `Hashed`; on mismatch delete the stale marker and return the existing `DigestMismatch`.
- Marker locations: URL models beside the blob (`<cache>/models/<key>/<name>.verified`, under the existing `lock_artifact` guard); path sources at `<cache>/markers/<source_cache_key(path)>.verified`.
- Wire-in: `ensure_blob` cache-hit branch ([artifacts.rs:230](promptforge/crates/promptforge-gateway/src/local/artifacts.rs:230)), the post-download verification in the same function, and the path-source branch of `ensure_model` ([artifacts.rs:120](promptforge/crates/promptforge-gateway/src/local/artifacts.rs:120)).

**Accepted trust tradeoff (code comment + commit message):** mtime+size is spoofable by anyone who can write the cache; the cache root is already operator-trusted. The pin still fully guards the download path - a marker is written only after a real hash match.

Tests (`local/artifacts/tests.rs`, tempfile fixtures): first run hashes and writes a correct marker; second run is a `MarkerHit`; changed content re-hashes and still raises `DigestMismatch`; wrong-pin/corrupt marker falls back to hashing; post-download success writes the marker.

## Commit 2: spawn llama-server at below-normal priority on Windows

In [local/server/support.rs](promptforge/crates/promptforge-gateway/src/local/server/support.rs:41), factor production `Command` construction into `production_command(request)` and apply:

```rust
/// Win32 BELOW_NORMAL_PRIORITY_CLASS. Raw value: stable ABI, avoids a
/// windows-sys dependency in the main build.
#[cfg(windows)]
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

#[cfg(windows)]
{
    use std::os::windows::process::CommandExt;
    command.creation_flags(BELOW_NORMAL_PRIORITY_CLASS);
}
```

- Respawns inherit it (same `ChildSpawner`, [server.rs:449](promptforge/crates/promptforge-gateway/src/local/server.rs:449)).
- Non-Windows: documented no-op.
- Unconditional, not config-gated.

**Corrected rationale (replaces the earlier overclaim):** this yields CPU and I/O scheduling to interactive processes during weight loading. It does NOT address the WDDM display-driver lock that freezes the desktop when the compute GPU is also the display GPU - the fix for that is the hardware change (driving monitors from on-board video or a secondary card), which the user is pursuing separately.

Test: `#[cfg(windows)]` test spawning `cmd /c exit 0` through `production_command`, asserting `GetPriorityClass == BELOW_NORMAL_PRIORITY_CLASS` via `windows-sys` as a dev-dependency only (`Win32_System_Threading`, `Win32_Foundation`).

## Commit 3 (CONDITIONAL): Gemma-4 dialect recognition

**Only if Step 0 proves the switch fails in dialect resolution.** In [local/dialect.rs](promptforge/crates/promptforge-gateway/src/local/dialect.rs):

- `fetch_props_evidence`: also read `props.chat_template_caps.supports_tool_calls` before falling back to the `/v1/models` probe.
- `openai_score`: add the Gemma-4 conjunction `template.contains("<|tool_call|>") && template.contains("<|tool_response|>")`.

Tests: Gemma-4 template markers resolve to `"openai"`; `/props` caps true resolves to `"openai"`; legacy Gemma-3 evidence still resolves to `gemma3_tool_code`.

If Step 0 clears dialect resolution, this commit is cancelled and the spike's actual finding is fixed in its place (plan updated at that point with what forced the change).

## Verification

- Rust-rulebook gates before every commit: `cargo fmt --all --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings`.
- Per commit: `cargo test --locked -p promptforge-gateway` with the step's focused filter (`local::artifacts` / `local::server` / `local::dialect`).
- Final Verify: `cargo test --locked --workspace --all-features`.
- Manual: next real profile switch - `starting-models` begins weight loading without a hash pass, and the switch to `gemma` either succeeds or fails with a captured, specific error.

## Out of scope

- On-board video / secondary display GPU (hardware change, user-handled).
- Defender exclusion for the cache dir (operator action).
- Completing or removing the partial `Qwen3.5-9B` download (operator action; the next switch to a profile containing it will resume it).
- Diffing old vs new model sets on switch to keep surviving children running.
- whisper worker priority in promptforge-ws-server.


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: digest_marker_and_child_priority_f3987e49

## Origin

The plan grew out of a live incident in the middle of an unrelated plan run (the chat opened with `coroutine_protocol_executor`). The user's problem report, verbatim:

"when I switch profiles in the gateway the machine makes a big thud. cursor ui becomes unresponsive, and other aspects of the machine become unresponsive. it comes back but there is a noticeable hiccup."

"my machine is beefy though. 96 core threadripper. 1TB RAM. how is it possible that the machine thuds? I can't even put text into an edit box. doesn't make sense. ANd, the profile switch failed."

So the plan carries two distinct problems from one event: the machine-wide stall, and the switch actually failing (generic red "Profile switch failed" in the UI, real error never captured).

## Why these fixes, in the user's words

The two performance fixes were the user's explicit choice, verbatim: "I want the digest-marker and I also want the lowered priority can we do that?"

The dialect commit was also a user directive. When offered a 2-commit performance plan with dialect work deferred, the user replied, verbatim: "add the dialect obviously"

Design thinking behind each:

- Digest marker: code reading showed `ensure_model`/`ensure_blob` re-hash every sha256-pinned GGUF on every cache hit - a full sequential read of 19-29 GB before llama-server even starts, then llama-server reads the same files again. The spike later measured the hash pass at ~8 of the 8.3 minutes of switch time, confirming it as the dominant cost of a switch.
- Child priority: the "thud" was diagnosed as GPU/WDDM contention plus scheduling, not CPU or RAM starvation - all 96 cores idle while the desktop compositor waits on the display driver. Below-normal priority makes weight loading yield to interactive processes.
- Dialect: Gemma-4's template uses new `<|tool_call|>` / `<|turn|>` markers that neither scorer in `dialect.rs` recognized.

## Discarded alternatives and corrections

1. Download failure as the switch-failure cause. The first hypothesis was a failed or 404 Hugging Face download of the Qwen/Gemma GGUFs. Discarded when the user pushed back, verbatim: "but it downloaded Gemma" and "but I have a 96gb blackwell". Live diagnosis then showed Gemma-4 loads in the pinned Vulkan llama-server in ~24 s with the gateway's exact flags, and `/props` reports `supports_tool_calls: true`.

2. Dialect resolution asserted as proven root cause, then walked back. The diagnosis session first concluded `resolve_local_dialect` returned `NoMatch` on Gemma-4's new tokens. The user's challenge, verbatim - "I am confused though I thought llama.cpp handled the dialect? I'm so confused." - forced a correction (paraphrase): llama.cpp with `--jinja` renders tools natively and reports the capability, and the gateway already reads `has_tool_call_capability` from `/v1/models`, so the existing code likely already resolves Gemma-4 to `"openai"`; the assistant stated "we have not proven dialect detection caused your failed switch". This is exactly why the plan has Step 0 as a gating spike and Commit 3 marked CONDITIONAL rather than fixing on an unproven root cause. (In execution the spike did capture `NoMatch`, unlocking Commit 3 - the original diagnosis turned out right, but the plan was structured so it did not have to be.)

3. Priority commit's overclaimed rationale. The original framing implied below-normal priority would fix the desktop freeze. Corrected in the plan: it only yields CPU and I/O scheduling; it does NOT address the WDDM display-driver lock that freezes the desktop when the compute GPU is also the display GPU. The real fix for that is hardware, and the idea came from the user, verbatim: "should I just use my on-board video of the motherboard?" The assistant endorsed on-board video or a cheap secondary display card; it is out of scope (user-handled).

4. Unix priority lowering. Considered and deferred: `nice` would need libc/`pre_exec` unsafe; the change is a documented no-op off Windows.

5. Deliberately out of scope per the chat: Defender exclusion for the cache dir (operator action), completing/removing the partial Qwen3.5-9B download, diffing old vs new model sets to keep surviving children running, whisper worker priority in promptforge-ws-server.

## Execution note (post-plan, same chat)

The plan's `windows-sys`-based priority test was replaced at run time with a PowerShell self-report probe, because the workspace lints forbid `unsafe_code` at forbid level - same coverage, zero new dependencies. Recorded as a rule-2 solo deviation in the ledger.
