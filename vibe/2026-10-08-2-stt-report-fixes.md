---
name: Fix STT report findings
overview: "Fix the promptforge gateway STT defects that the talktron evaluation report found and the source confirms: silent session exits, word loss in the overlap fallback, the PCM double count and causeless errors, term hints that never reach the realtime route, the realtime model name refused by batch, and the missing long-speech test. Each fix lands as its own commit with tests."
todos:
  - id: s6-silent-exits
    content: "S6: skip budget-starved captions; route the three silent returns in realtime/route.rs through end_session (log, error event, 1011 close); tests"
    status: pending
  - id: s1-overlap-guard
    content: "S1: forward-only punctuation snap, AlignmentFailure reason + metrics logged, sparse-successor density guard with PendingForced.text_start; adversarial tests"
    status: pending
  - id: s3-pcm-errors
    content: "S3: count each append once in pcm.rs try_append; BufferTooLong carries retained_ms and requested_ms; ClientError constructors take impl Into<String>; transcription_failed includes cause; replace finalization panic with TakeFailure::FinalTailEvicted"
    status: pending
  - id: s2-biasing
    content: "S2: prompt_terms helper; realtime guidance = config vocabulary + session prompt terms; batch honors prompt; update config doc and README; tests"
    status: pending
  - id: s5-model-name
    content: "S5: ModelNames::select maps realtime-transcribe to the final model; test"
    status: pending
  - id: s4-long-speech-test
    content: "S4: native realtime_long_speech test (no 5-word deletion run), fixture zip + hash, stt-miri.yml download and step"
    status: pending
  - id: verify-gates
    content: Run the full workspace gates (clippy with warnings denied, fmt, rustdoc, nextest) on the final code commit
    status: pending
isProject: false
---

# Fix the STT findings in promptforge

<product-contract>

## Product Requirements

The gateway's speech-to-text has six defects. An external evaluation found them on master and they are confirmed in the source. They cost talktron interview answers: realtime sessions end without a word, overlapping forced windows silently drop speech, buffer limits are refused with misleading or empty reasons, and term hints never reach the realtime route. This plan fixes each one in `crates/gateway/stt`, with tests in the same change.

- Problem and users:
  - The users are realtime and batch clients of the gateway's OpenAI-compatible STT routes. The main one is talktron, which streams interview answers from a browser microphone.
  - The defects come from the evaluation "PromptForge master STT against talktron's pinned gateway" by Will Pak, October 8, 2026 (https://gist.github.com/wpak-ai/f90698a7a2c04a58746bdc7f4cd13378). It measured promptforge at `fb8f85cc` against `3e917d20`.
  - Every finding used here was re-checked against the source at `fb8f85cc`. Local `master` is 8 commits ahead of that, and none of those commits touch `crates/gateway/stt`.
- Goals:
  - S6: no realtime session ends without an error event, a log entry and a close frame. A caption that the PCM budget cannot hold never ends a session.
  - S1: the forced-overlap fallback no longer drops words spoken before the successor's audio. A successor far sparser than its audio no longer replaces the previous window's text. Every alignment failure logs its reason.
  - S3: each append counts once against the 30 s PCM budget. Budget errors state required against actual. `transcription_failed` carries its cause. The final-tail panic becomes a named take failure.
  - S2: the config vocabulary and the client prompt reach both the realtime and batch routes. An oversized prompt loses only its trailing terms.
  - S5: the batch route accepts the `realtime-transcribe` name it advertises.
  - S4: a native CI test catches dropped spans of long speech when it runs the recommended models.
- Non-goals:
  - Talktron's own defects: its gateway entry model name, stereo decoding, sending hints as the session prompt, and capping append size.
  - Changing the 0.2 s sentence-end close.
  - Re-running the evaluation's measurements.
- Success criteria:
  - Every new test fails before its fix and passes after.
  - The workspace gates pass: build, formatter, linter with warnings denied, docs, and the full test suite.
  - The native long-speech test passes on the CUDA runner once its fixtures are published.
- Constraints:
  - Repository convention (`AGENTS.md`, Engineering): behavior changes ship with tests in the same change, and tests gate on yes/no properties, never on a score threshold.
  - Repository convention (`AGENTS.md`, Principles): error messages are concise and self-contained, and name required against actual.
  - Repository convention (`AGENTS.md`, Structural Rules): source directories are flat, a subdirectory needs three or more files, and one or two related files sit beside the parent as `foo-bar.rs` with a `#[path]` attribute.
  - Nothing is pushed. Local commits only.
  - Work happens in the linked git worktree `c:\Users\Vinnie\cursor\promptforge2`, on its current branch `vibe2`, and no branch is created. On 2026-10-08 `vibe2` was at `a1201da73`, the same commit as local `master`, with a clean worktree, no `vibe/ACTIVE`, and an empty diff against `master` under `crates/gateway/stt`. The main checkout `c:\Users\Vinnie\cursor\promptforge` stays untouched: run no git command there.
- Open questions: None

## Functional Specification

Each fix changes behavior a client can observe on the realtime WebSocket route or the batch transcription route, or changes what an operator sees in the gateway log. The specification below states that behavior per finding. The internal mechanics are in Technical Design.

- Actors and workflows:
  - A realtime client opens `/v1/realtime`, sends `session.update`, streams `input_audio_buffer.append` frames and commits. It receives hypothesis or delta events and a completed or failed transcript per item.
  - A batch client posts multipart audio with `model` and an optional `prompt`.
  - An operator reads the gateway log.
- Inputs and outputs:
  - Realtime `session.update` `transcription.prompt` is split into comma-separated terms. The gateway's `[stt] vocabulary` terms come first and the prompt terms follow. Both caption and final passes use the result.
  - The batch `prompt` field is used the same way, after the configured vocabulary.
  - Batch `model = "realtime-transcribe"` transcribes with the final model whenever a final model is configured.
- States and validation:
  - When the PCM budget cannot hold a caption window, that caption tick is skipped and the session stays open. The next tick tries again.
  - An append is admitted when retained audio plus the buffer's growth fits within 30 s. The incoming batch no longer counts a second time.
  - In the forced-overlap fallback, the cut never moves earlier than the projected point. A successor window whose word density is below one third of its predecessor's counts as sparse. In that case the predecessor's whole text is kept, and only the successor's projected tail past the overlap is used.
- Errors and recovery:
  - A session the gateway must end sends an `error` event with `type: server_error`, a code and a message naming the cause. The gateway then logs at error level and sends a WebSocket close frame with code 1011.
  - `too_much_unfinalized_audio` messages state the retained and requested durations and the 30 s limit.
  - The `transcription_failed` message ends with the take failure's own text, for example "Authoritative transcription failed: forced final window was not decodable".
  - When a final tail is no longer resident, the item fails with a named cause and the process does not panic.
  - Every overlap fallback logs why alignment failed.
- Security and privacy behavior: Log entries carry item ids, sample ranges, error codes and token counts. They carry no transcript text and no audio.
- Acceptance criteria:
  - A session whose caption copy exceeds the budget keeps running and later transcribes.
  - A forced session-loop failure produces an error event followed by a 1011 close, plus one error log line.
  - 20 s retained plus a 6 s append is accepted.
  - The geometry that lost "its place is to" keeps those words.
  - A 12.7 s successor decoding to 5 words, and one decoding to "The", both keep the predecessor's words.
  - A batch request with `prompt` passes its terms to the decode.
  - A batch request naming `realtime-transcribe` returns 200 when a final model is configured.

</product-contract>
<implementation-contract>

## Technical Design

Every change stays inside the gateway STT crates, apart from one config doc comment and the STT CI workflow. The visible changes are the realtime protocol's failure behavior (error event plus close frame), the text of error messages, and the routing of prompts and model names on the batch route. The rest is internal to the take's PCM budget and forced-window reconciliation.

- Architecture:
  - The realtime route loop is `run_socket` in `crates/gateway/stt/api/src/realtime/route.rs:134-200`.
  - It drives a `Session` (`crates/gateway/stt/api/src/realtime/session/route.rs`). The session owns a `Take` with a per-take `RetainedPcmBudget` (`crates/gateway/stt/api/src/take/pcm.rs`).
  - Forced final windows overlap by 8 s (`crates/gateway/stt/api/src/segment.rs:25`). The stride is about 10 s (`crates/gateway/stt/api/src/segment/endpoint.rs:41-42`).
  - Consecutive forced windows are reconciled in `record_forced_outcome` (`crates/gateway/stt/api/src/take/state.rs:333-404`). It tries token alignment first (`take/agreement-final-overlap.rs`), then falls back to a proportional projection (`take/agreement-projection.rs`).
  - Guidance terms reach whisper through `fit_glossary_counted` (`crates/gateway/stt/backend-whisper/src/prompt.rs:56-79`). Captions get 224 tokens. The final pass gets 112, followed by finalized-transcript history.
- Modules and interfaces:
  - S6 silent exits:
    - `run_socket` returns with no event, close frame or log at three points: `route.rs:170-172` (`catch_up_interim`, added on master in `189bde086`), `:178-180` (`finish_ready`), and `:186-188` (`schedule_interim`).
    - A new `end_session` helper in `route.rs` logs with `tracing::error!`, sends `session_error(..)` as an error event, and sends `Message::Close` with code 1011. All three exits go through it.
    - `Session::schedule_interim` (`session/route.rs:30-72`) maps `SessionError::Audio(AudioError::BufferTooLong { .. })` from `interim_window` (`take/interim.rs:28-47`) to a skipped tick. It logs at debug, matching the overloaded-worker skip at `session/route.rs:116-126`.
  - S1 alignment failure reason:
    - `range_guided_suffix_prefix_start` (`take/agreement-final-overlap.rs:66-81`) returns `Result<usize, AlignmentFailure>`, where `AlignmentFailure` carries a reason and the existing `AlignmentMetrics`.
    - The reasons cover every early return in `range_guided_suffix_prefix_start_with_metrics` (`:83-160`): transcript over the byte limit, token or normalization limits, no candidate, and ambiguous near-tied candidates.
  - S1 forward-only snap: in `locate_cut` (`take/agreement-projection.rs:101-152`), the punctuation candidate band starts at the projected token instead of 3 tokens before it.
  - S1 sparse successor:
    - In `record_forced_outcome`, after alignment fails, compare whitespace-token density per sample of the successor against the predecessor.
    - If the successor's density is below one third of the predecessor's, settle the predecessor's whole text over its whole range, and keep the successor text from the byte `projected_prefix_end(text, current_range, overlap.end)` returns.
    - Log this case as `warning_code = "forced_final_successor_sparse"`. Otherwise keep today's projection and log `forced_final_overlap_estimated` with the alignment reason added.
  - S1 pending text range:
    - `PendingForced` (`take/state.rs:82-85`) gains a text start, and a `text_range()` method returns text start through `boundary.decode_range().end`.
    - Every reader that pairs pending text with a range switches to `text_range()`:
      - the live-prefix snapshot (`state.rs:181-184`, `take/live_prefix.rs`)
      - the prior-window inputs in `record_forced_outcome`
      - `flush_pending_forced` (`state.rs:406-413`)
      - the test-only coverage (`state.rs:267-270`)
  - S3 single count:
    - In `RollingPcm::try_append` (`take/pcm.rs:201-252`), the incoming batch's capacity is reserved only on the empty-buffer adoption path (`:226-229`).
    - The growth path admits on headroom for growth alone.
    - The spare-capacity path needs no new reservation, because that capacity is already counted.
  - S3 error detail:
    - `AudioError::BufferTooLong` (`crates/gateway/stt/api/src/audio.rs:28`) gains `retained_ms` and `requested_ms`. Milliseconds keep the field unit-free across the input buffer (`audio.rs:77-96`) and the PCM budget (`pcm.rs:49`, `:202`, `:299`, `:305`).
    - `ClientError::request`, `overload` and `server` (`realtime/wire/vocabulary.rs` ~59-137) take `impl Into<String>`, since `ClientError.message` is already a `String`.
    - `session_error` (`route.rs:296-366`) formats the duration message.
    - `item_failure_error` (`realtime/wire/server-events.rs:111-129`) appends the stored `ItemFailure::TranscriptionFailed` text.
  - S3 panic: `take/finalization.rs:175-177` records a new `TakeFailure::FinalTailEvicted` ("final window audio was released before its decode") on the take, then sends the completion and breaks.
  - S2 prompt terms:
    - A shared `prompt_terms(prompt: &str) -> Vec<String>` in a new `crates/gateway/stt/api/src/guidance.rs` (declared in `api/src/lib.rs`) splits on commas, trims each term and drops empty ones. Both routes call it.
    - Realtime: `start_take` (`realtime/input.rs:147-160`) builds guidance as `engine.guidance()` (`generation-lease.rs:50-52`) followed by `prompt_terms(&snapshot.prompt)`. This replaces `Self::guidance` (`input.rs:162-168`).
    - Batch: `parse_form` keeps `prompt` (`batch.rs:211-215`) in `TranscriptionForm`, and `transcribe` (`batch.rs:122-142`) builds guidance as `generation.guidance()` followed by `prompt_terms(&form.prompt)`.
  - S5: `ModelNames::select` (`crates/gateway/stt/api/src/model.rs:65-73`) returns `DecodeMode::Final` for `REALTIME_TRANSCRIBE_MODEL` when `final_model` is set.
  - S4 test support:
    - The only native session driver is the private `Capture` in `crates/gateway/stt/api/tests/it/replay/native_capture.rs` (~107-296), behind the private `mod native_capture` in `tests/it/replay.rs`. Its clock is audio position plus measured decode time, not real-time sleeps.
    - Move its session setup into a shared test-support module under `crates/gateway/stt/api/tests/it/` that both the replay capture and the new long-speech test call.
    - The long-speech test adds real wall-clock pacing: 100 ms appends, each followed by a 100 ms sleep.
- File and public API changes:
  - Wire behavior: new error event plus 1011 close on gateway-ended sessions. Error message text changes for `too_much_unfinalized_audio` and `transcription_failed`. Error codes and types are unchanged.
  - Batch: `prompt` is honored, and `realtime-transcribe` is accepted.
  - Docs:
    - The `[stt] vocabulary` doc comment (`crates/gateway/config/src/config/stt.rs:23`) says it reaches both routes, ahead of the client prompt.
    - The STT README Prompt row (`crates/gateway/stt/README.md` ~58) and its PCM cap text (~69) describe the new behavior.
  - CI: `.github/workflows/stt-miri.yml` `native-whisper` job downloads `ggml-base.en.bin`, `ggml-small.en.bin` and the long-speech clip zip, each with a SHA256 check, exports their paths, and adds the test step.
- Data, persistence, failure, security, and privacy constraints:
  - No persisted format changes.
  - Log fields carry no transcript text.
  - The clip fixtures are LibriSpeech test-clean, licensed CC BY 4.0, from `https://www.openslr.org/resources/12/test-clean.tar.gz`.
    - Convert each clip from FLAC to 16 kHz mono 16-bit WAV with ffmpeg, which is installed on the development machine.
    - Pack the WAVs with a `clips.json` of `{ "id", "file", "text" }` entries into `stt-long-speech-1.zip`.
    - Start the clip selection from the evaluation's long clips: `2961-960-0010`, `3570-5695-0001`, `260-123288-0015`, `5142-36600-0001`, `3729-6852-0017` and `1320-122617-0007`. Replace any clip under 12 s or with numerals or number words in its reference.
    - Build the zip and its SHA256 in scratch space, outside the repository.
    - The zip is published as release asset `stt-long-speech-1` on `cppalliance/promptforge`, beside the existing `whisper-lib-b4938` asset (`.github/workflows/stt-miri.yml:66`). Only a maintainer with release rights can upload it.

</implementation-contract>
<verification-contract>

## Testing Plan

Each finding gets deterministic unit or integration tests that fail before the fix. The long-speech accuracy check is a native, ignored test that runs on the self-hosted CUDA runner. Its gate is a yes/no property, not a score.

- Unit:
  - S6:
    - A session whose take has a reduced PCM limit skips the caption and stays usable. This needs a test-fixtures constructor for a take with a smaller `RetainedPcmBudget`, which today is `pub(super)` in `take/pcm.rs`.
  - S1:
    - `locate_cut` never selects a token before the projected one. Update `ambiguous_or_distant_punctuation_keeps_the_projected_boundary` (`take/agreement-projection.rs:200-213`).
    - A synthetic predecessor over 0 to 10.27 s with a successor from 2.27 s keeps tokens 5 to 7.
    - A synthetic 2.0 to 14.7 s successor of 5 words after a 0 to 10 s, 36-token predecessor keeps all 36 predecessor tokens.
    - A successor of the single word "The" behaves the same way.
    - Each alignment failure reason is returned for its trigger.
  - S3:
    - `pcm-tests.rs` accepts 20 s retained plus a 6 s append.
    - The `BufferTooLong` message names the retained and requested durations.
    - `item_failure_error` includes the cause.
    - Update existing matches on `BufferTooLong { maximum_seconds: 30 }` in `audio-tests.rs`, `realtime/session/items-tests.rs` and `take/finalization/release_tests.rs`.
  - S2:
    - `prompt_terms` splits, trims and drops empty terms.
    - The realtime guidance test in `realtime/input.rs` (~264-278, which today asserts `["first"]`) now expects the config vocabulary followed by the prompt terms.
    - A `backend-whisper/src/prompt.rs` test shows an oversized comma list keeping its leading terms.
  - S5: `ModelNames::select("realtime-transcribe")` returns `Final` with a final model and `None` without one.
- Integration and end-to-end:
  - S6:
    - Add a `RoutePolicy` test fixture (`route.rs:43-92`) that forces the `finish_ready` branch to fail. Expose it on the STT service the way `fail_realtime_precommit` is exposed (`crates/gateway/stt/api/src/service.rs:146-149`).
    - A socket-level test in `crates/gateway/app/tests/it/realtime_stt/recovery.rs` asserts the error event, then the 1011 close. It uses the `server`, `connect` and `expect_type` helpers that `crates/gateway/app/tests/it/realtime_stt/overload.rs` uses.
  - S2: a batch test in `batch-tests.rs` shows `prompt` terms reaching the decode request.
  - S5: a batch test shows `realtime-transcribe` returning 200 when a final model is configured. It goes beside `an_unloaded_model_is_not_found` in `batch-tests.rs`.
  - S4:
    - The native, ignored test `realtime_long_speech` streams each clip at wall-clock pace through a production session with base.en for captions and small.en for finals.
    - It normalizes both texts: lowercase, punctuation removed, whitespace collapsed.
    - It aligns them by word edit distance and fails when any run of 5 or more consecutive reference words is missing.
    - Clips: six LibriSpeech test-clean clips of 12 s or more whose reference texts contain no numerals or number words.
- Regression, security, and performance: All existing STT unit, integration, Miri-filtered and replay tests keep passing, including the replay baseline gates in `tests/it/replay.rs`.
- Exit criteria:
  - Every new deterministic test fails on the parent commit and passes on its step's commit. The native long-speech test is exempt, because it needs CUDA-runner fixtures.
  - The full workspace gates pass on the final commit.
  - The native long-speech test compiles locally and is ignored by default.

</verification-contract>
<decision-record>

## Decision Record

These decisions settle the choices the evaluation left open, favoring the smallest change that removes each observed loss. Two report recommendations are deferred until there is measurement to support them.

- Decisions:
  - Honor the batch `prompt` instead of rejecting it. Talktron's fallback sends its hints there, so honoring them helps the path the evaluation saw lose answers. The user's words: "make a new plan that actually fixes things".
  - Put config vocabulary before client prompt terms. Truncation drops trailing terms first, so operator terms survive an oversized client prompt.
  - Split prompts on commas. The glossary rejoins terms with ", ", so a prompt that fits renders as it does today, and one that overflows keeps its leading terms instead of being dropped whole.
  - Make the punctuation snap forward-only. Snapping earlier than the projected point drops words neither window holds. Snapping later can at worst repeat up to 3 words.
  - Set the sparse-successor threshold at one third of the predecessor's density. In the evaluation's worst cases the density ratio was about one seventh, and normal pacing variation stays well above one third.
  - Use a deletion-run property (5 or more consecutive reference words missing) as the long-speech gate. It targets the silent-drop failure and is yes/no.
  - Use milliseconds for the `BufferTooLong` detail fields. The input buffer and the PCM budget count in different units.
  - Close gateway-ended sessions with WebSocket code 1011 (internal error).
  - Run the work in the `promptforge2` worktree on branch `vibe2`, not in the main checkout. The user's words: "yeah lets move it to promptforge2, which is at the same commit". The worktree is at the same commit as `master`, and an earlier plan there (`mcp-client-plugin`) was closed in `020292701`, so nothing else holds it. The commits land on `vibe2`, which tracks `origin/vibe2`. They reach `master` by a later fast-forward or merge, which this plan does not perform.
- Rejected alternatives:
  - Rejecting the batch `prompt` with a 400 error. It would break talktron's fallback outright. Revisit if a client depends on the prompt being ignored.
  - Leaving silent exits in place and only adding logs. The client still could not tell why its session ended. Revisit never.
  - Treating the whole session prompt as one glossary term. That is today's behavior, and it drops 307-character hint strings entirely from the final pass. Revisit never.
- Assumptions, risks, and notes:
  - Behavior change: deployments that set `[stt] vocabulary` will now see it bias realtime captions and finals too. Until now it affected batch only (`batch.rs:131-137`).
  - The forward-only snap can repeat up to 3 words at a forced-window seam where it previously dropped them.
  - The one-third density threshold is a judgment call until the long-speech test runs. Confidence is medium, because it rests on few clips.
  - The native long-speech test needs the fixture zip uploaded as a release asset. Until it is uploaded, a CI step would fail on download. So the workflow change is its own last work item, and it starts only after the operator confirms the upload. The test itself lands earlier, ignored by default and skipped when `PROMPTFORGE_LONG_SPEECH_CLIPS` is unset.
  - The small.en model download is about 466 MB per uncached runner job.
  - Confidence: S6, S3 and S5 high (small changes, defects confirmed in code). S1 medium (sound mechanism, untuned threshold). S2 high on mechanics, medium on recognition gain. S4 medium (external asset and runner time).

### Deferred and Out of Scope

- Deferred: changing the 0.2 s sentence-end close (`segment/endpoint.rs:29`, `:197-199`). Removing it raised the evaluation's errors from 51 to 66. Revisit once the long-speech test runs, to compare a two-caption agreement rule or a 0.6 s close.
- Deferred: letting alignment skip an unaligned successor head (`take/agreement-final-overlap.rs:118-131`). Short heads already pass within the 20% edit budget, and the sparse-successor guard covers the large-loss case. Revisit if the long-speech test shows a seam loss with a dense successor.
- Out of scope: talktron fixes (gateway entry model name, mono decoding, sending hints as the session prompt, append size cap).
- Out of scope: re-running the evaluation's measurement runs.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked` builds only the default member `crates/gateway/app` (package `gateway`, default features `local`, `web-search`, `config-ui`, `stt`). Build one crate with `cargo build --locked -p <crate>`, for example `-p gateway-stt`. Headless shape gate: `cargo check -p gateway --no-default-features` (drops `gateway-stt` entirely). A fresh clone needs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first because UI bundles build in crate build scripts (`.github/workflows/ci.yml`). Aliases in `.cargo/config.toml`: `cargo xtask` runs `build-xtask`, `cargo workshop` runs `build-workshop`. Windows links with `rust-lld` and static CRT. Toolchain is stable (`rust-toolchain.toml`).
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-substring>` (or `-E 'test(<regex>)'`), for example `cargo nextest run --locked -p gateway-stt --all-features take::finalization`. Native STT tests are `#[ignore]` and need fixture env vars (`PROMPTFORGE_WHISPER_LIBRARY`, `PROMPTFORGE_WHISPER_MODEL`, `PROMPTFORGE_WHISPER_AUDIO`, `PROMPTFORGE_SILERO_MODEL`, optional `PROMPTFORGE_WHISPER_BACKEND`); run them as `cargo test --locked -p <crate> --test <name> -- --ignored --test-threads=1` (`.github/workflows/stt-miri.yml`). Miri: `cargo +nightly-2026-09-05 miri test -p gateway-stt --features test-fixtures miri_` and the same for `-p gateway-stt-engine`; Miri tests are named `miri_*`. Local tools: cargo-nextest 0.9.128, cargo-deny 0.20.2, cargo-hakari 0.9.38, Node 24.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`; the STT subsystem is `-p gateway-stt -p gateway-stt-engine -p gateway-stt-backend-whisper -p gateway-whisper-ffi`, and its route tests through the gateway are `cargo nextest run --locked -p gateway --all-features realtime_stt`. `.config/nextest.toml` puts `gateway-stt` and `gateway-stt-backend-whisper` in the `heavy` test group (`threads-required = 4`) and gives `gateway` `realtime_stt::noise::` and `realtime_stt::capture::` a longer timeout. Structural and boundary checks: `cargo test -p build-xtask`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` (`AGENTS.md` Verification, `ci.yml`).
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, and for the trio `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets` (PowerShell: `$env:CARGO_BUILD_WARNINGS='deny'`). Never run a standalone `cargo check --workspace` beside clippy. `.githooks/pre-push` runs the headless check, the workspace clippy, and `cargo deny --log-level error check`.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml` sets `style_edition = "2024"`; `.githooks/pre-commit` runs it).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, plus `cargo doc -p promptforge --no-deps`, `cargo doc -p harness --no-deps`, and `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`, all with `-D warnings` (`ci.yml`). Facade surface: `cargo +<pinned nightly> xtask api --check` against `crates/promptforge/public-api.txt`.
- Test placement and naming conventions:
  - STT unit tests sit beside their module as `<stem>-tests.rs` wired by `#[cfg(test)] #[path = "<stem>-tests.rs"] mod tests;` (`crates/gateway/stt/engine/src/worker.rs`, `crates/gateway/stt/api/src/audio.rs`, `crates/gateway/stt/whisper-ffi/src/vad.rs`), or inside the module directory as `tests.rs`, `tests-<label>.rs`, and `<topic>_tests.rs` (`crates/gateway/stt/api/src/take/` holds `tests.rs` and `detector_tests.rs`, declared from `take.rs` as `mod tests;` and `mod detector_tests;`; `take/finalization/` holds `release_tests.rs`, `short_tests.rs`, `speech_tests.rs`).
  - `gateway-stt` integration tests are one binary at `crates/gateway/stt/api/tests/it/main.rs` with topic modules and subdirectories (`realtime_session/`, `replay/`), shared helpers in `tests/common/`, and JSON data in `tests/fixtures/` (`realtime/`, `replay/` with paired `*.snapshots.json`). `gateway-stt-engine` and `gateway-stt-backend-whisper` use one file per test binary under `tests/` (`engine_contract.rs`, `native_whisper.rs`, `native_silero.rs`), and backend-whisper keeps `tests/fixtures/ggml-tiny.en.bin` and `jfk.wav`.
  - Gateway route tests live in `crates/gateway/app/tests/it/`; `realtime_stt.rs` (behind `#[cfg(feature = "stt")]` in `main.rs`) declares `capture`, `native`, and `noise` as modules and `include!`s the other `realtime_stt/*.rs` topic files.
  - Test-only scaffolding is exported through each STT crate's `test-fixtures` feature (`gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory, scripted_service}`), enabled from `[dev-dependencies]`.
  - Test functions are full snake_case behavior sentences, such as `blocked_server_send_expires_and_releases_admission`. Test binary roots open with `#![expect(clippy::expect_used, ..., reason = "...")]`.
- Directory map:
  - `crates/gateway/` (private container): `app` (package `gateway`, HTTP surface and tests), `cloud-providers`, `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and the nested `stt/` subsystem.
  - `crates/gateway/stt/`: `api` (package `gateway-stt`: `SpeechService` facade, artifacts, batch transcription, Realtime sessions under `realtime/`, segmentation under `segment/`, take state, agreement, finalization, and windowing under `take/`), `engine` (package `gateway-stt-engine`: decoder and detector traits, worker threads, startup, policy), `backend-whisper` (package `gateway-stt-backend-whisper`: Whisper model, prompt, Silero detection, warm-up), `whisper-ffi` (package `gateway-whisper-ffi`: the dynamically loaded whisper.cpp C ABI). `engine` and `whisper-ffi` carry their own `AGENTS.md`.
  - Engine crates `crates/promptforge`, `crates/promptforge-plugin`, `crates/promptforge-internal/`; Harness crates `crates/harness`, `crates/harness-gateway-client`, `crates/harness-internal/runner`; Plugins `crates/plugin-web`, `crates/plugin-user-input`, `crates/plugin-mcp`; public gateway pair `crates/gateway-api-types`, `crates/gateway-api-discovery`; `crates/workshop/` (Workshop crates and npm UI packages); `crates/shared-*`; build crates `build-xtask`, `build-ceiling`, `build-ui`, `build-workshop`, `build-user-guide`, `build-llama-cuda`; `crates/workspace-hack` (cargo-hakari).
  - `guide/` (user guide sources), `prompts/`, `tools/`, `vibe/` (dated plan archive), `.github/workflows/` (`ci.yml`, `stt-miri.yml` with Miri and self-hosted Windows CUDA native Whisper jobs, `whisper-lib.yml`), `.githooks/`, `.config/` (`nextest.toml`, `hakari.toml`).
- Component boundaries (enforced by `cargo test -p build-xtask`, `crates/build-xtask/src/product.rs` and `tidy.rs`):
  - STT dependency direction: `gateway-stt` depends on `gateway-stt-backend-whisper`, `gateway-stt-engine`, `gateway-config`, `gateway-local`, and `gateway-progress`; `gateway-stt-backend-whisper` depends on `gateway-stt-engine`, `gateway-whisper-ffi`, and `gateway-progress`; `gateway-stt-engine` depends only on `thiserror`, `tokio`, and `workspace-hack`; `gateway-whisper-ffi` depends on `libloading`, `thiserror`, and `tracing`.
  - `crates/gateway/stt/` is a subsystem private to the gateway family with `gateway-stt` as its only public member; only the `gateway` app (optional `stt` feature) depends on it.
  - Gateway crates depend on no promptforge, workshop, or Harness crate; Workshop crates reach the gateway only through `gateway-api-types` and `gateway-api-discovery`. Engine crates depend on no gateway, workshop, or Harness crate. Outside crates reach the Engine only through `promptforge` and `promptforge-plugin`, and the Harness only through `harness` and `harness-gateway-client`.
  - Unsafe code is confined to `gateway-whisper-ffi`'s FFI boundary (its `AGENTS.md`; `build-xtask` unsafe allowlist). Decode workers are stateless and blocking decoders stay on their own threads (`crates/gateway/stt/engine/AGENTS.md`).
  - The Host installs Plugins: `crates/workshop/server/src/agents.rs` installs `plugin_web`, `plugin_user_input`, and per-entry `plugin_mcp` packages.
- Conventions summary:
  - Rust edition 2024 on stable, resolver 3. Every dependency is declared once in root `[workspace.dependencies]` with a comment explaining any pin or feature choice; members use `.workspace = true`, depend on `workspace-hack`, set `publish = false`, and inherit `[lints] workspace = true`.
  - Lints are strict: clippy `all` and `pedantic` deny, `unwrap_used` and `expect_used` deny, `allow` attributes banned in favor of `#[expect(..., reason = "...")]`, `unsafe_code` deny, `missing_docs` and `unreachable_pub` warn (fatal under the gate).
  - Every `lib.rs` opens with a `//!` crate doc; the `## Invariants` section is mandatory only for `workshop-*`, `harness-*`, and `plugin-*` crates (the STT crates have none; `gateway` has one).
  - Rust files stay at or under 500 lines (`build-ceiling`, run from every `build.rs`). Source directories are flat: one or two child files sit beside the parent as `foo-bar.rs` with `#[path]`, three or more become `foo/`.
  - Inside `crates/gateway/stt/`, a bare "engine" means the speech engine; Engine, Harness, Host, and Plugin are capitalized defined terms elsewhere (`AGENTS.md`).
  - Error and status messages are written for model consumption, naming required versus actual. Comments state non-obvious constraints and cite upstream issue URLs for workarounds (`whisper-ffi/AGENTS.md` cites whisper.cpp issue 3508). Behavior changes ship with tests in the same change.
  - JSON reaching a recorder or replay round-trips exactly (`serde_json` with `float_roundtrip`, sorted keys, finite numbers); STT replay fixtures pair `<name>.json` with `<name>.snapshots.json`.
  - Commit subjects are short imperative sentences with no prefix; a finished plan lands as `Close plan: <slug>`.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Report every gateway-ended realtime session (S6) [completed]

- Component: Realtime failure reporting
- Component placement: first. Its fixes are small and confirmed in code (high confidence). After it lands, every later failure seen while testing arrives as an error event plus a log line instead of a silent exit.
- Piece: S6 silent exits, the first of two sequential pieces. S3 follows in Step 2 because both edit `session_error` in `crates/gateway/stt/api/src/realtime/route.rs`.
- Artifacts:
  - `Session::schedule_interim` in `crates/gateway/stt/api/src/realtime/session/route.rs`: map `SessionError::Audio(AudioError::BufferTooLong { .. })` from `interim_window` (`crates/gateway/stt/api/src/take/interim.rs`) to a skipped caption tick with a `tracing::debug!` line, matching the overloaded-worker skip in the same file. The session stays open and the next tick tries again.
  - A test-fixtures constructor for a take with a smaller `RetainedPcmBudget`, which is `pub(super)` in `crates/gateway/stt/api/src/take/pcm.rs` today. Expose it only under `cfg(test)` or the `test-fixtures` feature.
  - An `end_session` helper in `crates/gateway/stt/api/src/realtime/route.rs` (420 lines today). It logs with `tracing::error!` (item id, error code, sample ranges, never transcript text or audio), sends `session_error(..)` as an `error` event with `type: server_error`, a code and a cause-naming message, then sends `Message::Close` with code 1011.
  - Route the three silent returns in `run_socket` through `end_session`: after `catch_up_interim`, after `finish_ready`, and after `schedule_interim`.
  - A `RoutePolicy` test fixture in `crates/gateway/stt/api/src/realtime/route.rs` that forces the `finish_ready` branch to fail, exposed on the STT service beside `fail_realtime_precommit` in `crates/gateway/stt/api/src/service.rs`.
- Tests:
  - Unit: a session whose take has the reduced PCM limit skips the caption tick, stays usable, and still transcribes after commit.
  - Socket: in `crates/gateway/app/tests/it/realtime_stt/recovery.rs`, using the `server`, `connect` and `expect_type` helpers that `overload.rs` uses, the forced `finish_ready` failure produces an `error` event and then a close frame with code 1011.
  - Log: a `gateway-stt` integration test drives the same forced failure and uses the `LogBuffer` writer in `crates/gateway/stt/api/tests/it/realtime_session.rs` to assert exactly one error-level line, with no transcript text in it.
- Verification: confirm each new test fails with the fix reverted. Run the STT crate tests (`cargo nextest run --locked -p gateway-stt -p gateway-stt-engine -p gateway-stt-backend-whisper -p gateway-whisper-ffi --all-features`) and `cargo fmt --all --check` and `cargo nextest run --locked -p gateway --all-features realtime_stt`.
- Commit: `Report gateway-ended realtime sessions and skip budget-starved captions` (local only)

</step-1>

<step-2>

### Step 2: Count PCM once and name causes in STT errors (S3) [completed]

- Component: Realtime failure reporting
- Piece: S3 PCM count and error detail, after S6 because both edit `session_error`.
- Artifacts:
  - `RollingPcm::try_append` in `crates/gateway/stt/api/src/take/pcm.rs` (435 lines): reserve the incoming batch's capacity only on the empty-buffer adoption path. The growth path admits on headroom for the growth alone. The spare-capacity path reserves nothing, because that capacity is already counted.
  - `AudioError::BufferTooLong` in `crates/gateway/stt/api/src/audio.rs` gains `retained_ms` and `requested_ms` next to `maximum_seconds`. Fill them at every construction site in `crates/gateway/stt/api/src/audio.rs` (input buffer) and `crates/gateway/stt/api/src/take/pcm.rs` (PCM budget).
  - `ClientError::request`, `overload` and `server` in `crates/gateway/stt/api/src/realtime/wire/vocabulary.rs` take `impl Into<String>`.
  - `session_error` in `crates/gateway/stt/api/src/realtime/route.rs` formats `too_much_unfinalized_audio` as the retained and requested durations against the 30 s limit.
  - `item_failure_error` in `crates/gateway/stt/api/src/realtime/wire/server-events.rs` appends the stored `ItemFailure::TranscriptionFailed` text, for example "Authoritative transcription failed: forced final window was not decodable".
  - `TakeFailure::FinalTailEvicted` in `crates/gateway/stt/api/src/take/state.rs` with the text "final window audio was released before its decode". In `crates/gateway/stt/api/src/take/finalization.rs` (around lines 175-177), replace the panic: record the failure on the take, send the completion, and break.
  - Update the existing matches on `BufferTooLong { maximum_seconds: 30 }` in `crates/gateway/stt/api/src/audio-tests.rs`, `crates/gateway/stt/api/src/realtime/session/items-tests.rs` and `crates/gateway/stt/api/src/take/finalization/release_tests.rs`.
  - `crates/gateway/stt/README.md` PCM cap text (around line 69) describes the single count.
- Tests:
  - `crates/gateway/stt/api/src/take/pcm-tests.rs`: 20 s retained plus a 6 s append is accepted.
  - The `BufferTooLong` message names the retained and requested durations and the 30 s limit.
  - `item_failure_error` includes the take failure's text.
  - In `crates/gateway/stt/api/src/take/finalization/release_tests.rs`, a final tail that is no longer resident fails the item with `FinalTailEvicted`, without a panic.
- Verification: confirm each new test fails with the fix reverted. Run the STT crate tests (`cargo nextest run --locked -p gateway-stt -p gateway-stt-engine -p gateway-stt-backend-whisper -p gateway-whisper-ffi --all-features`) and `cargo fmt --all --check` and `cargo nextest run --locked -p gateway --all-features realtime_stt`.
- Commit: `Count PCM appends once and name causes in STT errors` (local only)

</step-2>

<step-3>

### Step 3: Send prompt terms to both STT routes (S2) [completed]

- Component: Prompt and model routing
- Component placement: second. It does not depend on the other components. It lands before the long-speech test so that runs before and after Step 6 differ only by the S1 change.
- Piece: S2 prompt terms, the first of two sequential pieces. S5 follows in Step 4 because both edit `crates/gateway/stt/api/src/batch.rs` and `crates/gateway/stt/api/src/batch-tests.rs`, and S5's batch test runs through the form this step reshapes.
- Artifacts:
  - A new `crates/gateway/stt/api/src/guidance.rs`, declared in `crates/gateway/stt/api/src/lib.rs`, with `prompt_terms(prompt: &str) -> Vec<String>`. It splits on commas, trims each term and drops empty ones. Tests sit beside it as `guidance-tests.rs` wired with `#[path]`.
  - `start_take` in `crates/gateway/stt/api/src/realtime/input.rs`: build guidance as `engine.guidance()` (`crates/gateway/stt/api/src/generation-lease.rs`) followed by `prompt_terms(&snapshot.prompt)`. Remove `Self::guidance`.
  - `parse_form` in `crates/gateway/stt/api/src/batch.rs` keeps `prompt` in `TranscriptionForm`. `transcribe` builds guidance as `generation.guidance()` followed by `prompt_terms(&form.prompt)`.
  - The `[stt] vocabulary` doc comment in `crates/gateway/config/src/config/stt.rs` (line 23) says the vocabulary reaches both routes, ahead of the client prompt.
  - `crates/gateway/stt/README.md` Prompt row (around line 58) describes the new behavior.
- Tests:
  - `prompt_terms` splits, trims and drops empty terms.
  - The realtime guidance test in `crates/gateway/stt/api/src/realtime/input.rs` (around lines 264-278, which asserts `["first"]` today) expects the config vocabulary followed by the prompt terms.
  - A test in `crates/gateway/stt/backend-whisper/src/prompt.rs` shows an oversized comma list keeping its leading terms through `fit_glossary_counted`.
  - A test in `crates/gateway/stt/api/src/batch-tests.rs` shows `prompt` terms reaching the decode request.
- Verification: confirm each new test fails with the fix reverted. Run the STT crate tests (`cargo nextest run --locked -p gateway-stt -p gateway-stt-engine -p gateway-stt-backend-whisper -p gateway-whisper-ffi --all-features`) and `cargo fmt --all --check` and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features -p gateway-config`.
- Commit: `Pass config vocabulary and client prompt terms to both STT routes` (local only)

</step-3>

<step-4>

### Step 4: Accept realtime-transcribe on the batch route (S5) [completed]

- Component: Prompt and model routing
- Piece: S5 model name, after S2 because both edit the batch files.
- Artifacts:
  - `ModelNames::select` in `crates/gateway/stt/api/src/model.rs`: return `DecodeMode::Final` for `REALTIME_TRANSCRIBE_MODEL` when `final_model` is set, and `None` when it is not.
- Tests:
  - Unit: `ModelNames::select("realtime-transcribe")` returns `Final` with a final model and `None` without one.
  - A batch test beside `an_unloaded_model_is_not_found` in `crates/gateway/stt/api/src/batch-tests.rs` shows `realtime-transcribe` returning 200 when a final model is configured.
- Verification: confirm each new test fails with the fix reverted. Run the STT crate tests (`cargo nextest run --locked -p gateway-stt -p gateway-stt-engine -p gateway-stt-backend-whisper -p gateway-whisper-ffi --all-features`) and `cargo fmt --all --check`.
- Commit: `Accept the realtime-transcribe model name on the batch route` (local only)

</step-4>

<step-5>

### Step 5: Add the native long-speech deletion test (S4 test) [completed]

- Component: Long-speech regression test
- Component placement: third. It must come before Step 6 so it can run on the commits before and after the S1 guard. It comes after Steps 1-4 so those runs see sessions that report their failures and receive prompt terms.
- Piece: the shared driver and the test are built together in one step. The extracted driver has no new behavior of its own, and the replay suite plus the new test cover it together.
- Artifacts:
  - A new shared test-support module under `crates/gateway/stt/api/tests/it/` (for example `native_session.rs`, declared in `crates/gateway/stt/api/tests/it/main.rs`). It holds the session setup moved out of the private `Capture` in `crates/gateway/stt/api/tests/it/replay/native_capture.rs` (450 lines). `Capture` calls the shared module.
  - A new `crates/gateway/stt/api/tests/it/long_speech.rs` with the `#[ignore]` test `realtime_long_speech`:
    - It reads `PROMPTFORGE_LONG_SPEECH_CLIPS`, a directory holding `clips.json` and the WAVs, and returns early when it is unset.
    - It uses the env names `native_capture.rs` already reads: `PROMPTFORGE_WHISPER_LIBRARY`, `PROMPTFORGE_WHISPER_MODEL` for captions (base.en) and `PROMPTFORGE_WHISPER_FINAL_MODEL` for finals (small.en).
    - It streams each clip through a production session as 100 ms appends, each followed by a 100 ms wall-clock sleep.
    - It normalizes reference and transcript: lowercase, punctuation removed, whitespace collapsed.
    - It aligns them by word edit distance and fails when any run of 5 or more consecutive reference words is missing, naming the clip id and the run.
  - Fixture zip, built in scratch space outside the repository and never committed:
    - Source: LibriSpeech test-clean (CC BY 4.0) from `https://www.openslr.org/resources/12/test-clean.tar.gz`.
    - Start from `2961-960-0010`, `3570-5695-0001`, `260-123288-0015`, `5142-36600-0001`, `3729-6852-0017` and `1320-122617-0007`. Replace any clip under 12 s or whose reference has numerals or number words, keeping six clips.
    - Convert each FLAC to 16 kHz mono 16-bit WAV with ffmpeg. Write `clips.json` as `{ "id", "file", "text" }` entries. Pack everything into `stt-long-speech-1.zip` and compute its SHA256.
    - Record the final clip ids, the asset name `stt-long-speech-1.zip` and its uppercase SHA256 in the native-tests part of `crates/gateway/stt/README.md`, next to the `PROMPTFORGE_LONG_SPEECH_CLIPS` description. Step 7 reads the hash from there.
    - Tell the operator where the zip is, so a maintainer can upload it as release asset `stt-long-speech-1` on `cppalliance/promptforge`.
  - If native fixtures are available locally, run the test once on this commit and record any deletion runs as the baseline for Step 6.
- Tests: `realtime_long_speech` compiles and is ignored by default. The existing replay tests in `crates/gateway/stt/api/tests/it/replay.rs`, including the baseline gates, still pass after the extraction. This native test is exempt from the fail-before-fix rule.
- Verification: run the STT crate tests, and `cargo test --locked -p gateway-stt --test it -- --ignored --list` shows `realtime_long_speech`.
- Commit: `Add a native long-speech deletion test for realtime STT` (local only)

</step-5>

<step-6>

### Step 6: Keep predecessor words when forced-overlap alignment fails (S1) [completed]

- Component: Forced-overlap reconciliation
- Component placement: fourth. It follows the long-speech test so the native run can compare before and after. It is the last code component because its threshold has medium confidence, and its commit is the one that runs the full workspace gates.
- Piece: the failure reason, forward-only snap, pending text range and sparse-successor guard are built together in one commit, because the guard needs the failure reason and the text range. Build order inside the step: failure reason, snap, text range, guard.
- Artifacts:
  - `AlignmentFailure` in `crates/gateway/stt/api/src/take/agreement.rs` (130 lines; `agreement-final-overlap.rs` is 472 lines and near the ceiling). It carries a reason and the existing `AlignmentMetrics`. The reasons cover every early return in `range_guided_suffix_prefix_start_with_metrics`: transcript over the byte limit, token limit, normalization limit, no candidate, and ambiguous near-tied candidates.
  - `range_guided_suffix_prefix_start` in `crates/gateway/stt/api/src/take/agreement-final-overlap.rs` returns `Result<usize, AlignmentFailure>`.
  - `locate_cut` in `crates/gateway/stt/api/src/take/agreement-projection.rs`: the punctuation candidate band starts at the projected token, never before it.
  - `PendingForced` in `crates/gateway/stt/api/src/take/state.rs` (474 lines) gains `text_start`. A `text_range()` method returns text start through `boundary.decode_range().end`. Switch every reader that pairs pending text with a range to `text_range()`: the live-prefix snapshot (`state.rs` around lines 181-184 and `crates/gateway/stt/api/src/take/live_prefix.rs`), the prior-window inputs in `record_forced_outcome`, `flush_pending_forced`, and the test-only coverage (around lines 267-270).
  - The sparse-successor guard, called from `record_forced_outcome` after alignment fails, placed in `crates/gateway/stt/api/src/take/state/reconcile.rs` (64 lines) to keep `state.rs` under the ceiling:
    - Compare whitespace-token density per sample of the successor against the predecessor.
    - Below one third: settle the predecessor's whole text over its whole range, keep the successor text from the byte `projected_prefix_end(text, current_range, overlap.end)` returns, and log `warning_code = "forced_final_successor_sparse"`.
    - Otherwise: keep today's projection and log `forced_final_overlap_estimated` with the alignment reason added.
    - Log fields carry item ids, sample ranges, codes and token counts, never transcript text.
- Tests, in `crates/gateway/stt/api/src/take/state/alignment_tests-adversaries.rs`, `crates/gateway/stt/api/src/take/agreement-final-overlap-tests.rs` and the projection tests:
  - Update `ambiguous_or_distant_punctuation_keeps_the_projected_boundary`. `locate_cut` never picks a token before the projected one.
  - A predecessor over 0 to 10.27 s with a successor from 2.27 s keeps tokens 5 to 7 (the "its place is to" geometry).
  - A 2.0 to 14.7 s successor of 5 words after a 0 to 10 s, 36-token predecessor keeps all 36 predecessor tokens.
  - A successor of the single word "The" behaves the same way.
  - Each `AlignmentFailure` reason is returned for its trigger.
- Verification:
  - Confirm each new test fails with the fix reverted. Run the STT crate tests (`cargo nextest run --locked -p gateway-stt -p gateway-stt-engine -p gateway-stt-backend-whisper -p gateway-whisper-ffi --all-features`) and `cargo fmt --all --check`.
  - If native fixtures are available locally, rerun `realtime_long_speech` and compare with the Step 5 baseline.
  - Run the full workspace gates on this commit: `cargo build --locked`, `cargo check -p gateway --no-default-features`, `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and the same clippy for `-p workshop -p workshop-server -p workshop-server-api --all-targets`, `cargo fmt --all --check`, the rustdoc commands with `-D warnings` from the Project Survey, both full-suite nextest commands, and `cargo test -p build-xtask`.
- Commit: `Keep predecessor words when forced-overlap alignment fails` (local only)

</step-6>

<step-7>

### Step 7: Run the long-speech test on the CUDA runner (S4 CI)

- Component: Long-speech CI job
- Component placement: last. Do not start until the operator confirms the `stt-long-speech-1` release asset is uploaded on `cppalliance/promptforge`. Before that, the download would fail every run. It needs the zip SHA256 that Step 5 recorded in `crates/gateway/stt/README.md`.
- Piece: one piece, the workflow change.
- Artifacts:
  - `.github/workflows/stt-miri.yml`, `native-whisper` job, "Provision pinned native fixtures" step:
    - Download `ggml-base.en.bin` and `ggml-small.en.bin` from `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/`, and `https://github.com/cppalliance/promptforge/releases/download/stt-long-speech-1/stt-long-speech-1.zip`.
    - Check each against a pinned uppercase SHA256, the way the existing fixtures are checked. Get the model hashes by downloading each once and running `Get-FileHash`.
    - Expand the zip, then export `PROMPTFORGE_WHISPER_BASE_MODEL`, `PROMPTFORGE_WHISPER_SMALL_MODEL` and `PROMPTFORGE_LONG_SPEECH_DIR` through `$env:GITHUB_ENV`. Leave `PROMPTFORGE_WHISPER_MODEL` at tiny.en for the existing steps.
    - Do not export `PROMPTFORGE_LONG_SPEECH_CLIPS` job-wide. Otherwise the existing "Test native Gateway STT integration" step, which runs every ignored `it` test with tiny.en, would run `realtime_long_speech` with the wrong models.
  - A new step, "Test native long-speech transcription", with `shell: powershell`. Inside its own `run`, it sets:
    - `$env:PROMPTFORGE_LONG_SPEECH_CLIPS` to `$env:PROMPTFORGE_LONG_SPEECH_DIR`
    - `$env:PROMPTFORGE_WHISPER_MODEL` to the base path
    - `$env:PROMPTFORGE_WHISPER_FINAL_MODEL` to the small path

    It then runs `cargo test --locked -p gateway-stt --test it realtime_long_speech -- --ignored --test-threads=1`.
- Tests: no Rust changes. The CI run on the CUDA runner is the test, and it happens when the operator pushes.
- Verification: the YAML parses, each pinned hash matches a local download, and the env var names match what `realtime_long_speech` reads.
- Commit: `Run the native long-speech test on the CUDA runner` (local only)

</step-7>

Before the run's first commit, the session that runs the steps checks `c:\Users\Vinnie\cursor\promptforge2`. Nothing here is committed:

- Confirm `vibe2` is checked out, the worktree is clean, `a1201da73` is an ancestor of `HEAD`, and `vibe/ACTIVE` is absent. When `vibe/ACTIVE` names this plan, the run is resuming and skips this check.
- Build output for the STT crates may be cold in this worktree. The first `cargo` run can rebuild whisper's native dependencies, so allow for that before the first test run.

</execution-plan>
