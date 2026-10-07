---
name: STT debt removal
overview: "Remove the five debts kept after the Debt Collector's analysis and challenge of upstream/master..HEAD in promptforge (a dead error path, a masked text-loss hazard in released-range settlement, a redundant text-only silence veto, an unread wire extension the Workshop negotiates, and a hand-edited native replay fixture), plus three rejected candidates the user added (test literals, overload-skip logging, the test-only result queue) and the user's decision to remove the loudness fallback so speech fails loudly when Silero cannot run."
todos:
  - id: d1-5-overload-path
    content: "D1-5 and D1-32: remove the orphaned result_queue_overload path and the whole test-only result queue"
    status: pending
  - id: remove-fallback
    content: "Remove the loudness fallback: Silero is required; provisioning or load failure leaves speech unavailable, a take that cannot open or run Silero fails with an error event; delete FallbackDetector, FallbackReport, Activity::hub, and the native fallback recorder (supersedes D1-14, D1-18, D1-29)"
    status: pending
  - id: d1-23-test-constants
    content: "D1-23: tests use SPEECH_TAIL_SAMPLES, HANGOVER_SAMPLES, and SENTENCE_END_SILENCE_SAMPLES instead of literals"
    status: pending
  - id: d1-10-overload-log
    content: "D1-10: debug log for interim ticks skipped under worker overload"
    status: pending
  - id: d1-2-released-guard
    content: "D1-2: add SPEECH_TAIL_SAMPLES <= HANGOVER_SAMPLES assertion and released-range boundary tests; clip by close kind only if the stride test fails"
    status: pending
  - id: d1-1-phrase-veto
    content: "D1-1: add Bye./you echo tests, trial-remove SILENCE_HALLUCINATIONS, verify with noise test and scored captures"
    status: pending
  - id: d1-4-ranges-request
    content: "D1-4: Workshop stops requesting and decoding the hypothesis ranges extension; update wire fixture tests"
    status: pending
  - id: d1-3-native-fixture
    content: "D1-3: re-capture jfk native replay as jfk-native-recaptured, point NATIVE_FIXTURE at it, document no hand edits"
    status: pending
  - id: exit-checks
    content: "Exit: full build, fmt, lint, docs, full suite, Miri, native noise test, and scored captures"
    status: pending
isProject: false
---

# PromptForge Debt Removal: STT Two-Model and Silero Work

<product-contract>

## Product Requirements

- Scope and target work
  - Repository `c:\Users\Vinnie\cursor\promptforge`, branch `engine-names-only-caller`. Target `3e917d209..19c32230f`: baseline `upstream/master` (`3e917d209`, local ref as last fetched), endpoint and disposition `19c32230f`, clean worktree.
  - The 55 target commits are:
    - Engine caller naming: `28bb8f7ab` through `0a367c8f8`.
    - The STT two-model upgrade: `a5ea520db` through `f5a0ade20`, plan `vibe/2026-10-06-1-stt-two-model-upgrade.md`.
    - Unplanned STT fixes: `8e617095c` through `ec0368dda`.
    - A CSS tweak: `5ed2b8a8a`.
    - The Silero plan: `afddac002` through `19c32230f`, plan `vibe/2026-10-07-1-silero-speech-detection.md`.
- Cleanup goals
  - Remove debts D1-1, D1-2, D1-3, D1-4, and D1-5 as the work items below specify.
  - Also fix three rejected candidates the user added to scope: D1-10, D1-23, and the pre-existing D1-32.
  - Remove the loudness fallback at the user's direction. Silero becomes a required speech artifact, and speech fails loudly instead of silently degrading. This also removes D1-14, D1-18, and D1-29, which existed only in fallback code.
- Non-goals
  - No wire format, config schema, or public API change. The gateway keeps the `item.input_audio_transcription.hypothesis.ranges` extension for other clients.
  - No redesign of skipped or released range settlement, and no change to the tuned speech constants or the Silero thresholds.
- Success criteria
  - Every work item's check passes, and so do the full suite, lint, docs, and Miri for `gateway-stt`.
  - The gateway-level captures score no worse than the tuned Silero capture recorded in the Silero plan's Decision Record: final punctuation 10 of 11 lines on dictation-01 and 2 of 4 on JFK, word errors 1 and 0, and no unsaid text during pauses. The native noise test shows no text.
  - No production take ever detects speech by loudness.
    - When the Silero model cannot be provisioned or opened at speech load, speech is unavailable and the cause is reported, exactly as for a Whisper model failure.
    - When a take cannot open or run its Silero detector, the client receives an error event and already-finalized text stays.

## Functional Specification

### Debt Inventory

- Debt added
  - D1-1 (introduced): text-only silence heuristics were corrected twice for removing real speech, and one of them now overlaps the audio-level gate.
    - Evidence: `cf6ab23b8` added `SILENCE_HALLUCINATIONS` with six phrases. `53f4a2307` cut it to three after it hid a spoken "Thank you." and "Bye." and suppressed the sentence-end hint. `ee1434132` added the echo cut, and `d542f5081` corrected it after it removed "you." from "Thank you.".
    - Present state: the three-phrase veto remains at `crates/gateway/stt/backend-whisper/src/guard.rs:14` and is applied at `:82`.
    - Relationship to target work: the Silero plan now ends interim windows 100 ms after detected speech (`take/interim.rs:13`) and skips final decodes with no detected speech (`200d8f96d`). That is the same cause handled at the audio level.
    - Impact: a text-only guess that has misfired before and may now catch nothing.
    - Reversal cost: cheap.
    - Target state: the phrase veto is removed if captures show it is redundant, and the two untested short phrases are pinned by tests.
  - D1-2 (worsened): skipped and released range settlement needed four corrections (`8e617095c`, `a84b31b6d`, `0edd4267c`, `ec0368dda`) after `0b4891d71` added `ClosedRange::Released`.
    - Mechanism: a released range reads `accepted_hypotheses(range.end)` without the clipping `a84b31b6d` added for skipped ranges (`take/finalization.rs`, `record_outcome`). Text from a window that ends past the range end is therefore dropped from the completed transcript.
    - History: this was live from `200d8f96d` (300 ms speech tail against a 100 ms hangover) until `eb067da83`. It is masked today only because `SPEECH_TAIL_SAMPLES` (`take/interim.rs:13`) happens to equal `HANGOVER_SAMPLES` (`segment/endpoint.rs:45`).
    - Reachability: release requires sustained overload, meaning a full final queue plus the 30 s retained PCM cap.
    - Reversal cost: moderate and internal to the crate.
    - Target state: a retune that breaks the masking fails to compile, and boundary tests pin released settlement.
  - D1-3 (introduced): replay fixture inputs restate the window spans the scheduler and segmenter compute, so timing changes forced edits to them.
    - The native fixture `jfk-native.json` was captured in `33ae08d37`, then hand-edited in `24a2a2502` (first final `sample_start` 960 to 0) and in `271954776` (a tick moved from 10060 to 10048 ms, a final from 32960..160960 to 32768..160768). This contradicts the two-model plan's rule that it "is a fixed input: later steps never overwrite it".
    - Its frozen baseline section is now compared against moved inputs. Under the aggregate gate, per-fixture commit lag rose above 110 percent on `scripted-final-word`, `scripted-short-word-final`, `scripted-trailing-echo`, and `scripted-trailing-echo-same-pass`, and `jfk-native` partial latency rose 12 percent.
    - Reversal cost: moderate, test-only.
    - Target state: an honestly captured native fixture is the required native replay, and the docs forbid hand edits.
  - D1-4 (introduced, prospective): the Workshop negotiates a public wire extension it never reads.
    - `1265fab64` added the server extension. `33e04140e` made the Workshop request it (`crates/workshop/ui/src/services/realtime-transcription.ts:257`) and decode `finalized_through_ms` and `finalized_seq` (`realtime-event-decoder.ts`), but the take registry reads neither field. No plan names a consumer.
    - Impact: the first-party adoption of a public token makes the server extension costly to reshape (the two-model decision record notes that "a public wire name is hard to change once clients adopt it"), and range-only changes send no-op revisions.
    - Target state: the Workshop does not request or decode the extension.
- Cheap fixes
  - D1-5: `077dee039` removed the only production producer of `result_queue_overload`.
    - What survives only in test builds: `SessionError::InterimAtCapacity` (`realtime/session/state.rs:54`), its `From<MailboxError>` mapping (`:67`), and the two arms in `realtime/route.rs:331-341`.
    - What still lists the code as a canonical wire case: `tests/fixtures/realtime/invalid-sequences.json:140` and the `INVALID_SEQUENCE_CASES` entry in `tests/it/realtime_fixtures/sequences.rs:51`.
    - No plan or `Deferred:` trailer reserves it.
- Exposed pre-existing debt: none found.
- Rejected candidates the user added to scope (polish with no demonstrated consequence, fixed because the plan already touches this code)
  - D1-10 (residual): when the speech worker is overloaded, an interim tick is skipped silently (`crates/gateway/stt/api/src/realtime/session/route.rs:103`, from `8ef9db761`), leaving no trace for diagnosing overload.
  - D1-14 (weak, superseded by the fallback removal): `generation-lease.rs:11` imports `crate::take::FallbackReport`, while `Take::new` takes a `GenerationLease`. That is an intra-crate module cycle from `5bf220e4e`, and it disappears with `FallbackReport`.
  - D1-18 (residual, superseded by the fallback removal): `FallbackReport` (`take/fallback.rs`) holds its progress activity in a `OnceLock` until the take drops, so the busy indicator stays on for the rest of a take that fell back to loudness.
  - D1-23 (residual): tests spell the timing constants as literals: the sentence-end silence as `3_200` in the `segment/endpoint.rs` tests (lines 256-377), the hangover as `const HANGOVER: u64 = 1_600` in the `take/window/echo.rs` tests (line 99), and the speech tail as `1_600` in `tests/it/replay/repeats.rs:95-96`. A retune such as a 0.3 s sentence-end close therefore edits several tests.
  - D1-29 (residual, superseded by the fallback removal): the native gateway tests detect a loudness fallback by matching the word "loudness" in warning text (`crates/gateway/app/tests/it/realtime_stt/native.rs:171-181`). Without a fallback, a missing Silero fails the service load or the take, so the matcher is no longer needed.
  - D1-32 (unrelated pre-existing): the test-only result queue in `realtime/result_mailbox.rs` predates the target. It consists of `SESSION_RESULT_CAPACITY`, `MailboxError::ResultAtCapacity`, `push_delta`, `replace_hypothesis`, and their queue fields, all behind `cfg(any(test, feature = "test-fixtures"))`. It sits next to D1-5.
- User-directed design change: remove the loudness fallback
  - Origin: the Silero plan's decision "Fall back to today's loudness segmenter and report it", implemented in `ab763d786`, `8c04ed76a`, and `5bf220e4e`.
  - Present code:
    - `FallbackDetector` and its `take_fault` in `crates/gateway/stt/engine/src/detector.rs`, used as the `Segmenter` and `TakeState` detector type (`segment.rs:39`, `take/state.rs:97-135`).
    - `FallbackReport` (`take/fallback.rs`) and the fallback branches in `GenerationLease::speech_detector` (`generation-lease.rs:60-77`).
    - The mid-take fault report (`take.rs:142`).
    - `SileroModel = Result<PathBuf, Arc<LocalError>>` and the "continue without the model" branch in `artifacts-silero.rs:16, 33-58`, plus the progress hub copied in `artifacts.rs:132`.
    - `Activity::hub` in `crates/gateway/progress/src/lib.rs:145`.
    - `recording_fallbacks` and `LoudnessFallbacks` in `crates/gateway/app/tests/it/realtime_stt/native.rs`, used by `capture.rs` and `noise.rs`.
  - Why remove it:
    - Silero comes through the same download path as the Whisper models at a fraction of their size (885,098 bytes), and missing VAD symbols already fail the whisper library load. Its realistic failures are therefore a cache file removed mid-run or an out-of-memory graph allocation.
    - Loudness dictation is much worse: on the DEMAND noise clips it invented 84 lines where Silero invented none, and on JFK it never closed a segment.
    - Silent degradation hid a real harness bug during the Silero run.
- Rejected candidates still out of scope: 22.
  - 13 residual-but-acceptable: deliberate, documented, or test-only, with no demonstrated consequence (lock order, unbounded `held`, test clones, and similar).
  - 4 weak or speculative: no reachable path or measured impact (per-take model load cost, digest recheck, `openInZone` throw, and the challenger's N1 on final-decode loudness gates, which the Silero plan keeps on purpose).
  - 5 false: the code or committed metrics contradict the claim (late-decode hint clearing is unreachable, `heard_speech` invariant holds, DOM-type naming allowed, `baseline.json` never edited, replay gates pass at the endpoint).

</product-contract>
<implementation-contract>

## Technical Design

- D1-1 (`crates/gateway/stt/backend-whisper/src/guard.rs`): remove `SILENCE_HALLUCINATIONS`, its check, and its tests. Keep the confidence veto and loop collapse. The echo cut in `crates/gateway/stt/api/src/take/window/echo.rs` and `repeats_final` in `take/state/reconcile.rs` are unchanged.
- D1-2 (`crates/gateway/stt/api/src/segment.rs`): add `const _: () = assert!(SPEECH_TAIL_SAMPLES <= HANGOVER_SAMPLES);` beside the existing assertion at line 28. Import `SPEECH_TAIL_SAMPLES` from `crate::take`. Add a one-line comment on the constraint: released ranges keep only accepted windows that end inside them.
- D1-3 (`crates/gateway/stt/api/tests/`):
  - Add `fixtures/replay/jfk-native-recaptured.json` and its `.snapshots.json`, copied unchanged from a fresh native replay capture.
  - `NATIVE_FIXTURE` in `it/replay.rs:27` names the new fixture.
  - `jfk-native.json` and its baseline section stay as frozen history.
  - The module docs of `it/replay.rs` and `it/replay/native_capture.rs` state that native fixtures are re-captured under a new name and never hand-edited.
- D1-4 (`crates/workshop/ui/`):
  - `realtime-transcription.ts` sends `include: [HYPOTHESIS_INCLUDE]` and drops its `requestedRanges` state (lines 101, 227, 242, 257, 320).
  - `realtime-event-decoder.ts` drops the following, keeping today's non-negotiated behavior, in which an event carrying range fields is rejected:
    - `HYPOTHESIS_RANGES_INCLUDE` (line 4)
    - `RealtimeDecodeOptions` and its `requestedRanges` paths (lines 8-9, 37, 166-189, 293, 310, 414)
    - `RANGE_FIELDS`
    - the `finalized_through_ms` and `finalized_seq` event fields (lines 128-130)
  - The gateway extension and its Rust wire tests are unchanged.
- D1-5 and D1-32 (`crates/gateway/stt/api/`): remove the orphaned overload error path and the whole test-only result queue.
  - D1-5: `SessionError::InterimAtCapacity` (`realtime/session/state.rs:54`) and its `From<MailboxError>` mapping (`:66-67`), both `realtime/route.rs` arms (`:331-341`), the `result_queue_overload` case in `tests/fixtures/realtime/invalid-sequences.json:140-142`, and its entry in `tests/it/realtime_fixtures/sequences.rs:51`.
  - D1-32: `SESSION_RESULT_CAPACITY`, `MailboxError::ResultAtCapacity`, `push_delta`, `replace_hypothesis`, and their gated queue fields in `realtime/result_mailbox.rs`, plus the mailbox unit test that fills the queue (`:202-237`). Also the wrappers in `realtime/session/items.rs:99-116`, the fixture methods in `test_fixtures/session.rs:98-115`, and the integration test `result_capacity_hypothesis_replacement_and_terminal_reservation_are_independent` (`tests/it/realtime_session/commit.rs`).
  - If any removed test also checks production behavior, such as the terminal-event reservation, keep that coverage through production paths before deleting it.
- Loudness fallback removal, which supersedes D1-14, D1-18, and D1-29:
  - Provisioning (`artifacts-silero.rs`): any failure other than cancellation fails the speech load with a `SpeechError` naming the cause, the way a Whisper model failure does. `SileroModel` becomes the verified path. Remove the "continue without the model" warning and progress text.
  - Speech load (`generation.rs`, `generation-snapshot.rs`):
    - The prepared generation always carries Silero (path plus `SileroSource`). The optional Silero field and its progress hub are removed.
    - At load, open one detector through `SileroSource::load` to prove the model and library work, then drop it. A failure fails the load.
    - A failed load leaves speech unavailable through the existing path (`SessionError` "speech generation is unavailable", boot keeps serving).
  - Detector type (`crates/gateway/stt/engine/src/detector.rs`): delete `FallbackDetector` and `take_fault`.
    - `Segmenter` (`segment.rs`) and `TakeState` (`take/state.rs`) hold a `Box<dyn SpeechDetector>`.
    - `Segmenter::classify` and `TakeState::classify` return the `DetectorError` instead of switching to loudness, and no chunk is ever classified by loudness in production.
    - `EnergyDetector` moves behind the engine's `test-fixtures` feature for scripted runtimes, `test_fixtures/segment.rs`, and `native_capture/audio.rs`.
  - Takes:
    - `GenerationLease::speech_detector` returns `Result<Box<dyn SpeechDetector>, DetectorError>`; scripted runtimes supply a test detector.
    - A take whose detector cannot open does not start, and the client gets an error event.
    - A detector error mid-take fails the take through the same error event; already-finalized text stays.
    - Reuse the error code a failed decode already sends, and add no new wire code.
    - Constructors without a lease (`take.rs:62`, `take/state.rs:97, 115`) become test-only.
  - Delete:
    - `take/fallback.rs` (`FallbackReport`) and its fields in `Take`
    - the progress hub plumbing for takes (`artifacts.rs:132`)
    - `Activity::hub` and its test in `crates/gateway/progress`
    - `recording_fallbacks` and `LoudnessFallbacks` in `crates/gateway/app/tests/it/realtime_stt/native.rs`, with their uses in `capture.rs` and `noise.rs`
  - Keep:
    - The native harness's held temporary cache (Step 10 of the Silero plan) and the orphan-scan exclusion (Step 6): takes still open the model file.
    - `EnginePolicy::is_silence` as the second gate on final decodes and in the whisper backend.
  - Update the doc comments that describe the fallback.
- D1-23 (tests only): replace literals that mean a timing constant with the constant.
  - `SENTENCE_END_SILENCE_SAMPLES` replaces `3_200` in the `segment/endpoint.rs` tests.
  - `HANGOVER_SAMPLES` replaces `const HANGOVER: u64 = 1_600` in the `take/window/echo.rs` tests. Widen its visibility to `pub(crate)` if needed.
  - `SPEECH_TAIL_SAMPLES` replaces `1_600` in `tests/it/replay/repeats.rs:95-96`, reached through a `test_fixtures` re-export.
  - Leave append chunk sizes and JSON script data alone; they are not timing constants.
- D1-10 (`realtime/session/route.rs:103`): the `TranscribeError::Overloaded` arm emits one `tracing::debug!` naming the skipped tick's window, then keeps today's behavior: clear `last_interim_end` and return `Ok(None)`.

</implementation-contract>
<verification-contract>

## Testing Plan

- Native recipes (CUDA, cached production models under `~/.promptforge/models/`)
  - Capture: set the following, then run `cargo nextest run --locked -p gateway --all-features --run-ignored only --no-capture -E 'test(native_realtime_capture_records_every_server_event_at_real_time_pace)'`:
    - `PROMPTFORGE_WHISPER_BACKEND=cuda`
    - `PROMPTFORGE_WHISPER_MODEL` to `ggml-base.en.bin` and `PROMPTFORGE_WHISPER_FINAL_MODEL` to `ggml-small.en.bin`
    - `PROMPTFORGE_WHISPER_AUDIO` to `local/stt-fixtures/dictation-01.wav` or `jfk.wav`
    - `PROMPTFORGE_REALTIME_CAPTURE` to a file outside the repository
  - Scoring: run `node --test test/stt-capture-score.mjs` from `crates/workshop/ui` with these set:
    - `PROMPTFORGE_REALTIME_CAPTURE`: the capture file
    - `PROMPTFORGE_CAPTURE_LINES`: `local/stt-fixtures/dictation-01-lines.json` or `jfk-lines.json`
    - `PROMPTFORGE_CAPTURE_SCORE`: an output path
  - Noise: the same model variables plus `PROMPTFORGE_NOISE_CLIPS=local/stt-fixtures/noise`, then `cargo nextest run --locked -p gateway --all-features --test it --run-ignored only --test-threads 1 realtime_stt::noise::`.
- D1-1
  - New echo tests (`take/window/echo.rs` tests or `tests/it/replay/repeats.rs`) check that "Bye." and a bare "you" after a sentence are never cut.
  - After the veto is removed, the noise test passes and both captures meet the success criteria. If any capture shows "please subscribe", "thanks for watching", or "thank you for watching" as unsaid text, restore the veto and record the hit.
- D1-2
  - `cargo build -p gateway-stt` fails when `SPEECH_TAIL_SAMPLES` is set above `HANGOVER_SAMPLES`. Check this once by hand, then revert.
  - New tests in `take/finalization/tests.rs`:
    - A silence-closed range released under the PCM cap, whose last accepted window ends exactly at the range end, keeps its interim text final.
    - A released forced stride whose accepted windows extend past the stride end loses no words from the completed transcript. If this test fails, fix it with close-kind-aware clipping in `record_outcome`: silence closes clip as `record_skipped_segment` does, and forced strides leave the overlap to the successor.
- D1-3
  - Diffing the committed `jfk-native-recaptured*.json` files against the scratch capture output shows no differences.
  - `cargo nextest run --locked -p gateway-stt --all-features replay` passes.
  - `git diff -- crates/gateway/stt/api/tests/fixtures/replay/baseline.json` shows only added lines.
- D1-4
  - `git grep -E 'finalized_through_ms|finalized_seq|hypothesis\.ranges' -- crates/workshop/ui/src` returns nothing.
  - A Workshop test asserts that the transcription service's `session.update` includes only `HYPOTHESIS_INCLUDE`.
  - `crates/workshop/ui/test/realtime-wire-fixtures.mjs` expects the Workshop decoder to reject range-carrying events. Workshop npm tests and typecheck pass.
  - Live dictation in the Workshop still works (manual check).
- D1-5 and D1-32
  - `git grep -E 'result_queue_overload|InterimAtCapacity|ResultAtCapacity|SESSION_RESULT_CAPACITY|push_delta|replace_hypothesis' -- crates` returns nothing.
  - `cargo nextest run --locked -p gateway-stt --all-features` passes, including the canonical invalid-sequence test and any retained terminal-reservation coverage.
  - Workshop `realtime-wire-fixtures.mjs` passes; it iterates `invalid-sequences.json` generically.
- Loudness fallback removal
  - `git grep -E 'FallbackDetector|FallbackReport|take_fault|LoudnessFallbacks|recording_fallbacks' -- crates` returns nothing, and `Activity::hub` is gone from `crates/gateway/progress`.
  - Artifact tests (from `8c04ed76a`): a digest mismatch and a failed fetch now fail the speech load with the cause, replacing the "leaves no path, reports once, and still loads" tests. Cancellation still fails the load as `InitialLoadCancelled`.
  - A generation load whose `SileroSource` fails to open, scripted, fails the load, and speech stays unavailable. Extend the boot test that keeps serving with speech unavailable (`crates/gateway/app/src/boot/boot_speech_tests.rs`) to cover a Silero provisioning failure.
  - Take tests (`take/detector_tests.rs`, replacing the fallback tests from `5bf220e4e`), each with a scripted primary:
    - A take whose detector fails to open does not start.
    - A detector error mid-take fails the take, keeps finalized text, and classifies no chunk by loudness afterward.
  - A realtime session test shows both failures reach the client as the existing decode-failure error event.
  - Engine: `cargo check -p gateway-stt-engine` without features builds no `EnergyDetector`; the Miri tests for the removed fallback are deleted.
  - Native: the capture and noise tests still pass on Silero. A missing model now fails `native_speech_service_with` at load, which the tests already treat as failure.
- D1-23: with `SENTENCE_END_SILENCE_SAMPLES` temporarily set to 0.3 s (4,800 samples), the endpoint, echo, and repeat tests pass without editing any test; revert the constant afterward. Replay goldens are expected to change under a real retune and are excluded from this check.
- D1-10: extend the existing overload-skip session test (from `8ef9db761`) with the shared tracing capture fixture to assert one debug event per skipped tick, and that the skip behavior is unchanged.
- Exit: the full build, formatter, linter, docs, and full suite; the gateway app suite; Miri for `gateway-stt` in WSL; the native noise test and both scored captures.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences
  - D1-1:
    - Trial-remove only the phrase veto.
    - The echo cut already requires detector speech evidence, and the Silero plan keeps it with a revisit trigger. The confidence veto reads decoder no-speech statistics. Loop collapse guards decoder loops that also occur during speech. `repeats_final` changes only the display.
    - Rejected: gating the veto on detector silence. Interim windows now always hold detected speech, so the gated veto would never fire.
  - D1-2:
    - Guard with a compile-time assertion and boundary tests rather than change settlement now.
    - Rejected: close-kind-aware clipping up front (more logic in a mechanism slated for a word-level redesign), and the word-level redesign itself.
  - D1-3:
    - Re-capture under a new name.
    - Rejected:
      - Storing a hash of each fixture input beside its baseline section. That is a structural ratchet on unpublished test data, which these rules reserve for stable product or security contracts with user approval.
      - Letting the replay driver accept whatever windows the take schedules. That removes the divergence check that caught the `33ae08d37` repair.
      - Overwriting `jfk-native.json` in place. That breaks the fixed-input rule again.
  - D1-4:
    - Stop the first-party request, which is a one-component change that is easy to undo.
    - Rejected: building a Workshop consumer (feature work that no plan names), and removing the gateway extension (a hard-to-reverse public wire change).
  - D1-5 and D1-32:
    - Remove the orphaned error path together with the pre-existing test-only queue behind it, so no gated remnant is left.
  - Loudness fallback removal:
    - Prove the model at speech load by opening one detector, so an unreadable file fails before the first dictation rather than at a take.
    - Reuse the decode-failure error event for take failures rather than add a wire code.
    - Keep `EnergyDetector` only as a test fixture, because scripted runtimes and the native replay capture still need a detector without a Silero model.
    - Rejected:
      - Bundling the Silero model inside the whisper runtime archive, which would change the pinned runtime bundle for no gain over the shared download path.
      - Holding one VAD context per generation, which would need a pool because contexts are `Send` but not `Sync` and sessions run concurrently.
  - D1-10:
    - Log at debug level, not warn, because overload skips are expected under load and finals still deliver text.
- User-resolved choices
  - The user added D1-10, D1-23, and the pre-existing D1-32 to scope from the rejected candidates.
  - The user chose to remove the loudness fallback: Silero is required, and speech fails hard and loud rather than degrading.
    - This reverses the Silero plan's fallback decision.
    - It supersedes the user's earlier picks of D1-14 and D1-29, and the timed notice chosen for D1-18.
    - The user also declined tinting the mic meter as too much investment.
  - Trade-off accepted: a machine with the Whisper models cached but not the Silero model cannot dictate until the 885,098-byte download succeeds once.
  - No chosen remediation is hard to reverse: no wire format, config schema, or public interface changes, and the fallback can be restored from history.
- Assumptions and risks
  - `upstream/master` is the local tracking ref as last fetched.
  - D1-4 assumes no Workshop behavior depends on range-only revisions. The take registry (`take-registry-events.ts`) reads only `revision`, `finalized`, `agreed`, `tentative`, and `transcript`.
  - D1-1's evidence comes from one speaker and one microphone.
  - The native replay capture runs on a scripted runtime with the test-only `EnergyDetector`, so the re-captured fixture's speech runs are loudness runs.
  - Removing the fallback assumes Silero failures are as rare as the code paths suggest. If dictation becomes unavailable in practice, the cause appears in load progress and logs.

### Deferred and Out of Scope

- Word-level coverage for skipped and released settlement (the two-model plan's Step 14 design). Revisit when the interim word-end work reserved by the `94386b591` and `3fdf7f001` `Deferred:` trailers lands.
- Close-kind-aware clipping for released ranges. Revisit if the forced-stride release test fails, or if the speech tail must exceed the hangover.
- A Workshop consumer for `finalized_through_ms` and `finalized_seq`. Revisit when a plan names one, and request the extension again then.
- Running the native replay capture on Silero. Revisit if replay fixtures need real Silero speech runs.
- Per-fixture replay thresholds in place of the aggregate rule. Revisit if a per-fixture regression reaches users.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked` builds the default member, package `gateway` (`crates/gateway/app`, binary `promptforge-gateway`), with its default features including `stt`; `cargo build --locked -p gateway --no-default-features` is the headless shape. CI runs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` before any cargo build because build scripts bundle the UIs.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <filter>`; add `--test it` for a crate's `tests/it` binary or `--test <stem>` for a flat target (for example `--test native_silero`). Native tests are `#[ignore]`d; run them with `--run-ignored only --test-threads 1` (nextest rejects `-- --ignored`; the CI native job uses the `cargo test ... -- --ignored --test-threads=1` form). They read `PROMPTFORGE_WHISPER_LIBRARY`, `PROMPTFORGE_WHISPER_MODEL`, `PROMPTFORGE_WHISPER_AUDIO`, `PROMPTFORGE_SILERO_MODEL`, optional `PROMPTFORGE_WHISPER_FINAL_MODEL`, `PROMPTFORGE_NOISE_CLIPS` for noise tests, and on the gateway path `PROMPTFORGE_WHISPER_BACKEND` (`cpu` or `cuda`). Local fixtures live in gitignored `local/stt-fixtures/`: whisper and CUDA DLLs, `ggml-tiny.en.bin`, `ggml-silero-v6.2.0.bin`, `jfk.wav`, `dictation-01.wav`, their `-lines.json` and `-loudness-baseline.json` files, and `noise/*.wav`; production models are cached under `~/.promptforge/models`. Native noise test: `cargo nextest run --locked -p gateway --all-features --test it --run-ignored only -E 'test(native_realtime_shows_no_text_for_noise_clips)'`. Gateway-level capture: `cargo nextest run --locked -p gateway --all-features --run-ignored only --no-capture -E 'test(native_realtime_capture_records_every_server_event_at_real_time_pace)'` with `PROMPTFORGE_REALTIME_CAPTURE` naming an output file outside the repository; score it with `node --test test/stt-capture-score.mjs` from `crates/workshop/ui` with `PROMPTFORGE_REALTIME_CAPTURE`, `PROMPTFORGE_CAPTURE_LINES`, and `PROMPTFORGE_CAPTURE_SCORE` set. Native replay re-capture: the ignored test in `crates/gateway/stt/api/tests/it/replay/native_capture.rs` with `PROMPTFORGE_REPLAY_CAPTURE` naming a scratch `<name>.json`. Miri: `cargo +nightly-2026-09-05 miri test -p <gateway-stt|gateway-stt-engine> --features test-fixtures miri_`; plan records run it in WSL Ubuntu on this Windows machine. One JS test file: `node --test <file>` from its package directory.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`; the STT subsystem: `cargo nextest run --locked -p gateway-stt -p gateway-stt-engine -p gateway-stt-backend-whisper -p gateway-whisper-ffi --all-features`; the gateway app: `cargo nextest run --locked -p gateway --all-features`; Workshop server relay: `cargo nextest run --locked -p workshop-server`; structural checks: `cargo test -p build-xtask`; Workshop JS: `npm test --workspaces --if-present` from `crates/workshop` (the `ui` package runs `node --test "test/**/*.mjs" "src/**/*.test.mjs"`).
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, then `npm test --workspaces --if-present` in `crates/workshop` and `npm test` in `crates/gateway/config-ui/ui`.
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, `cargo check -p gateway --no-default-features`, `npm run typecheck --workspaces --if-present` in `crates/workshop`, plus `cargo deny check`, `cargo audit`, and `cargo hakari verify`. In PowerShell set `$env:CARGO_BUILD_WARNINGS='deny'`.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml`: `style_edition = "2024"`).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`; with the same flags CI also runs `cargo doc -p promptforge --no-deps`, `cargo doc -p harness --no-deps`, `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`, and `cargo doc --locked --no-deps -p workshop-server --document-private-items`. Facade surface: `cargo +nightly-2026-09-05 xtask api --check` (pin in `crates/build-xtask/src/api/toolchain.rs`).
- Test placement and naming conventions:
  - Unit tests in `#[cfg(test)] mod tests`, inline or in a sibling `<stem>-tests.rs` or `<stem>_tests.rs` wired by `#[path]` or a child module; integration tests in `tests/it/main.rs` with submodule trees (`crates/gateway/stt/api/tests/it/`, `crates/gateway/app/tests/it/realtime_stt/`, `crates/workshop/server/tests/it/realtime_relay/`); flat targets in `crates/gateway/stt/engine/tests/` and `crates/gateway/stt/backend-whisper/tests/` (`native_whisper.rs`, `native_silero.rs`, `native_interim_timing.rs`).
  - Snake_case sentence names; Miri-safe tests prefixed `miri_`; native tests carry `#[ignore = "requires packaged whisper, ... fixtures"]` reasons; STT doubles (`ScriptedDetector`, replay scripts) sit behind the `test-fixtures` feature in each crate's `src/test_fixtures/`.
  - Replay fixtures in `crates/gateway/stt/api/tests/fixtures/replay/` (`jfk-native.json`, `scripted-*.json`, each with `.snapshots.json`, plus `metrics.json` and a frozen `baseline.json`), selected by `NATIVE_FIXTURE` in `tests/it/replay.rs`; regenerate snapshots and metrics with `PROMPTFORGE_REPLAY_UPDATE=1`. Wire fixtures in `tests/fixtures/realtime/` (`invalid-sequences.json` holds `result_queue_overload`). Workshop JS tests are `crates/workshop/ui/test/*.mjs` (`realtime-wire-fixtures.mjs`, `stt-stream.mjs`, `stt-replay.mjs`, `stt-capture-score.mjs`, `take-registry*.mjs`).
- Directory map:
  - `crates/`: the gateway family (`crates/gateway/app`, `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, `cloud-providers`, and `stt/`), the Workshop family (`crates/workshop/desktop`, `server`, `server-api`, `ui`, `look`, `platform`, and supporting crates), Engine crates (`promptforge`, `promptforge-internal/*`), Harness crates (`harness`, `harness-*`, `harness-internal/*`), build tooling (`build-ceiling`, `build-xtask`, `build-ui`, `build-workshop`, `build-user-guide`, `build-llama-cuda`), `shared-*` helpers, and `workspace-hack` (cargo-hakari).
  - `crates/gateway/stt/`: `api` (package `gateway-stt`: artifacts and Silero provisioning, generation, realtime sessions and wire, takes with finalization, interim, and `take/fallback.rs`, the segmenter), `engine` (`gateway-stt-engine`: backend-neutral decoding on worker threads, `EnginePolicy`, `SpeechDetector`, `EnergyDetector`, `FallbackDetector`), `backend-whisper` (`gateway-stt-backend-whisper`: whisper backend, decode profiles, `guard.rs` text guards, `silero.rs`), `whisper-ffi` (`gateway-whisper-ffi`: runtime-loaded bindings for whisper.cpp `b4938`, including streaming VAD in `vad.rs`).
  - Speech-adjacent code outside STT: `crates/gateway/config/src/config/stt.rs` (speech config and the `SILERO_VAD_MODEL` pin), `crates/gateway/progress` (`Activity`, `ProgressHub`), `crates/gateway/app/tests/it/realtime_stt/` (native, noise, and capture tests; `native.rs` holds the `LoudnessFallbacks` recorder), `crates/workshop/ui/src/services/realtime-transcription.ts` and `realtime-event-decoder.ts` plus `src/parts/take/take-registry-events.ts` (the Workshop speech client).
  - Elsewhere: `vibe/` (plan records), `guide/` (user guide books and landing site), `prompts/` (sample prompts), `tools/` (Node scripts such as `stage-gateway-sidecar.mjs`), `.github/workflows/` (`ci.yml`, `stt-miri.yml`, release jobs), `.config/` (`nextest.toml`, `hakari.toml`), `local/` (gitignored developer fixtures and config).
- Component boundaries:
  - Only `gateway` depends on `gateway-stt`. `gateway-stt` depends on the speech engine, the whisper backend, `gateway-config`, `gateway-local`, and `gateway-progress`; the whisper backend depends on the speech engine, whisper-ffi, and `gateway-progress`; the speech engine and whisper-ffi name no workspace crate besides `workspace-hack` and `build-ceiling`.
  - The Workshop reaches speech only over the realtime wire through the Workshop server relay; it shares no Rust crate with the STT subsystem.
  - `unsafe_code` is denied workspace-wide; among STT crates only whisper-ffi relaxes it, keeping raw pointers behind Drop-owning wrappers and retargeting its ABI pin with its size assertions (`crates/gateway/stt/whisper-ffi/AGENTS.md`). The speech engine keeps decode jobs stateless and blocking work on its owning worker threads (`crates/gateway/stt/engine/AGENTS.md`).
  - `cargo test -p build-xtask` enforces product and container boundaries, the Workshop tier graph, `## Invariants` markers, and lint inheritance.
- Conventions summary:
  - Rust 2024 edition on stable; clippy `all` and `pedantic` denied along with `unwrap_used`, `expect_used`, and `allow_attributes`; lints suppressed only with `#[expect(..., reason = "...")]`; every crate's `build.rs` runs `build_ceiling::check()`, a 500-line file limit; flat sources with `#[path]` kebab siblings until a group reaches three files.
  - Inside `crates/gateway/stt/`, a bare "engine" means the speech engine; Engine, Harness, Host, and Plugin are reserved terms checked by `crates/workshop/ui/test/docs-claims.mjs`.
  - Comments state only non-obvious constraints; behavior changes ship with tests in the same change; error messages are written for model consumption; JSON reaching a replay comparison round-trips exactly.
  - Commits use short imperative subjects, and a plan closes with `Close plan: <slug>`; plan records live in `vibe/YYYY-MM-DD-N-<slug>.md`.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Remove the orphaned overload path and the test-only result queue [completed]

- Component: Realtime overload handling
- Component order: first of six. It has no dependencies, and it trims `SessionError`, the error routing in `realtime/route.rs`, and `realtime/result_mailbox.rs`, which Step 5 extends when detector failures reach the client.
- Piece: orphaned overload path (D1-5 and D1-32), built before the overload-skip trace. The two pieces are independent; removing dead session code first leaves Step 2 a smaller session surface.
- Work, all in `crates/gateway/stt/api`:
  - Before deleting anything, read `result_capacity_hypothesis_replacement_and_terminal_reservation_are_independent` (`tests/it/realtime_session/commit.rs:61`) and the queue-filling unit test in `realtime/result_mailbox.rs` (`:202-237`). Any assertion they make about production behavior, such as the terminal-event reservation, moves to a test that drives production paths.
  - `realtime/session/state.rs`: delete `SessionError::InterimAtCapacity` (`:54`) and the `From<MailboxError>` branch that maps `MailboxError::ResultAtCapacity` to it (`:66-67`).
  - `realtime/route.rs`: delete both arms at `:331-341`.
  - `realtime/result_mailbox.rs`: delete `SESSION_RESULT_CAPACITY`, `MailboxError::ResultAtCapacity`, `push_delta`, `replace_hypothesis`, their `cfg(any(test, feature = "test-fixtures"))` queue fields, and the queue-filling unit test.
  - Delete the wrappers in `realtime/session/items.rs:99-116` and the fixture methods in `src/test_fixtures/session.rs:98-115`.
  - Delete the `result_queue_overload` case in `tests/fixtures/realtime/invalid-sequences.json:140-142` and its `INVALID_SEQUENCE_CASES` entry in `tests/it/realtime_fixtures/sequences.rs:51`.
- Verify with the Testing Plan's D1-5 and D1-32 checks: the `git grep` returns nothing, `cargo nextest run --locked -p gateway-stt --all-features` passes, and `node --test test/realtime-wire-fixtures.mjs` passes from `crates/workshop/ui`.
- Commit: `Remove the orphaned result queue overload path`.

</step-1>

<step-2>

### Step 2: Log interim ticks skipped under worker overload [completed]

- Component: Realtime overload handling
- Piece: overload-skip trace (D1-10), built after Step 1 in the same realtime session module tree.
- Work, all in `crates/gateway/stt/api`:
  - `src/realtime/session/route.rs:103`: the `TranscribeError::Overloaded` arm emits one `tracing::debug!` naming the skipped tick's window, then keeps today's behavior: clear `last_interim_end` and return `Ok(None)`.
  - No shared tracing capture fixture exists in `gateway-stt`. Add `tracing-subscriber` (workspace version) to its dev-dependencies and a debug-level capture helper in the `tests/it` tree, modeled on `capture_logs` in `crates/gateway/protocol/src/upstream/tests.rs` (a `MakeWriter` buffer installed with `tracing::subscriber::set_default` on a current-thread runtime). Keep `cargo hakari verify` passing.
  - Extend `an_overloaded_interim_is_skipped_and_the_next_tick_decodes_its_window` (`tests/it/realtime_session/interim.rs:105`) to assert exactly one debug event per skipped tick and unchanged skip behavior.
- Verify with the Testing Plan's D1-10 check: `cargo nextest run --locked -p gateway-stt --all-features realtime_session` passes.
- Commit: `Log interim ticks skipped under worker overload`.

</step-2>

<step-3>

### Step 3: Stop the Workshop negotiating the hypothesis ranges extension

- Component: Workshop ranges negotiation
- Component order: second. It has no dependencies and touches only `crates/workshop/ui`, so it ships ahead of the long gateway work. The gateway extension and its Rust wire tests stay unchanged.
- Piece: one piece, one step. The request, the decoder, and their tests change together because the decoder's negotiated-ranges paths have no other caller.
- Work, all in `crates/workshop/ui`:
  - `src/services/realtime-transcription.ts`: send `include: [HYPOTHESIS_INCLUDE]` and drop the `requestedRanges` state (lines 101, 227, 242, 257, 320).
  - `src/services/realtime-event-decoder.ts`: drop `HYPOTHESIS_RANGES_INCLUDE` (line 4), `RealtimeDecodeOptions` and its `requestedRanges` paths (lines 8-9, 37, 166-189, 293, 310, 414), `RANGE_FIELDS`, and the `finalized_through_ms` and `finalized_seq` event fields (lines 128-130). An event carrying range fields stays rejected, as it is today without negotiation.
  - `test/realtime-wire-fixtures.mjs`: drop the ranges include from `negotiableIncludes` and expect the decoder to reject range-carrying events.
  - `test/stt-stream.mjs`: the production service test (around line 1038) asserts that `session.update` includes only `HYPOTHESIS_INCLUDE`. Rewrite the `client_ranges_update` test (from line 1096), which negotiates ranges, so it pins rejection of range-carrying events without a ranges request.
- Verify with the Testing Plan's D1-4 checks: the `git grep` over `crates/workshop/ui/src` returns nothing; `npm test --workspaces --if-present` and `npm run typecheck --workspaces --if-present` pass from `crates/workshop`. Live dictation in the Workshop is a manual operator check after the run, not a coder check.
- Commit: `Stop the Workshop requesting hypothesis ranges`.

</step-3>

<step-4>

### Step 4: Require Silero at provisioning and speech load

- Component: Required Silero
- Component order: third. It follows Step 1 so detector failures join an already trimmed realtime error path, and it precedes Steps 7 through 10 so their tests, captures, and the re-captured native fixture reflect the final detector wiring.
- Piece: speech load, first of three sequential pieces. Takes can drop the missing-model branch only once every production generation carries a proven Silero model.
- Work, all in `crates/gateway/stt/api/src` unless named:
  - `artifacts-silero.rs`: `provision` returns the verified path, and any failure other than cancellation fails the load with a `SpeechError` naming the cause, as a Whisper model failure does. `SileroModel` becomes the verified `PathBuf`. Remove the "continue without the model" warning and progress text. Replace `a_silero_digest_mismatch_leaves_no_path_reports_once_and_still_loads` and `a_failed_silero_fetch_leaves_no_path_reports_once_and_still_loads` with tests in which both fail the load with the cause. Keep `a_cancelled_silero_fetch_cancels_the_load` (`InitialLoadCancelled`).
  - `artifacts.rs`: the prepared generation's `silero` field holds the path. Tests that use the `NO_SILERO` pin (`:323-326`) expect the load failure. The `hub` field (`:132`) stays until Step 5, because `FallbackReport` still reads it.
  - `generation.rs` (`load_initial` and the build at `:307`) and `generation-snapshot.rs` (`Silero.model`): at load, open one detector through `SileroSource::load` on the verified path and drop it. A failure fails the load, and a failed load leaves speech unavailable through the existing path (`SessionError` "speech generation is unavailable", boot keeps serving). `publish_scripted_silero` takes the path.
  - `generation-lease.rs::speech_detector`: drop the `model: Ok(..)` match now that the model is always a path. The runtime's optional `silero` field stays for scripted runtimes until Step 5.
  - Tests: a scripted generation load whose `SileroSource` fails to open fails the load, and speech stays unavailable. In `crates/gateway/app/src/boot/boot_speech_tests.rs`, extend `a_failed_boot_speech_load_leaves_the_gateway_serving_without_speech` or add a sibling so a Silero provisioning failure also leaves the gateway serving with speech unavailable.
- Verify: `cargo nextest run --locked -p gateway-stt --all-features` and `cargo nextest run --locked -p gateway --all-features boot_speech` pass.
- Commit: `Fail the speech load when Silero cannot load`.

</step-4>

<step-5>

### Step 5: Fail the take on detector errors and delete the loudness fallback

- Component: Required Silero
- Piece: take detection, second. It relies on Step 4's guarantee and leaves the remaining loudness surface dead or test-only for Step 6.
- Work:
  - `crates/gateway/stt/engine/src/detector.rs` and `lib.rs`: delete `FallbackDetector`, `take_fault`, and their Miri tests (`miri_fallback_*` and `miri_energy_fallback_uses_loudness_and_never_faults`). Keep `miri_energy_detector_matches_the_silence_gate` and `miri_detector_errors_carry_their_source_message`.
  - `crates/gateway/stt/api/src/segment.rs` and `take/state.rs`: `Segmenter` and `TakeState` hold a `Box<dyn SpeechDetector>`. `Segmenter::classify` and `TakeState::classify` return the `DetectorError` and stop classifying instead of switching to loudness.
  - `take/state.rs`: add a `TakeFailure` variant carrying the `DetectorError`. It reaches the client through the existing transcription-failed path (`ItemResult::PrecommitTranscriptionFailed` and `ItemFailure::TranscriptionFailed`), the error code a failed decode already sends, with no new wire code. Already-finalized text stays.
  - `generation-snapshot.rs` and `generation.rs`: the runtime's `silero` field becomes required and loses its progress hub. `load_scripted` and `load_scripted_shared` supply a test `SileroSource` from `test_fixtures/generation.rs`.
  - `generation-lease.rs`: `speech_detector` returns `Result<Box<dyn SpeechDetector>, DetectorError>`, and the `crate::take::FallbackReport` import goes (D1-14).
  - `take.rs`: `Take::new` takes a `GenerationLease` and returns the detector open error. `Take::append` records a mid-take `DetectorError` as a take failure. The constructors without a lease (`take.rs:62`, `take/state.rs:97, 115`) become test-only. Delete `take/fallback.rs` (D1-18), the `fallback` field, and the take hub plumbing (`artifacts.rs:132`).
  - `realtime/input.rs` (`from_audio`): a detector that cannot open records a pending `TakeFailure`, and the take never starts. Confirm production sessions always carry a lease (`realtime/session.rs::new`, `realtime/session/state.rs:75`). Any production path still lacking one fails the take like an open failure rather than detecting by loudness.
  - Test-only call sites switch to `Box<dyn SpeechDetector>`: `realtime/input.rs::first_append_with_detector`, `realtime/session.rs:56`, `test_fixtures/replay-script.rs::speech_detector`, `test_fixtures/replay.rs`, `test_fixtures/segment.rs`, `realtime/session/route-tests-interim.rs`, `take/tests.rs`, `segment/tests.rs`, `take/interim-tests.rs`, and `take/finalization/speech_tests.rs`.
  - Tests: in `take/detector_tests.rs`, replace `a_silero_load_failure_falls_back_to_loudness_and_reports_once` and the other fallback tests. With a scripted primary, show that a take whose detector fails to open does not start, and that a mid-take detector error fails the take, keeps finalized text, and classifies no chunk by loudness afterward. A realtime session test shows both failures reach the client as the existing decode-failure error event.
  - Update the doc comments these files carry about the fallback, including `speech_detector`'s.
- Verify: `cargo nextest run --locked -p gateway-stt -p gateway-stt-engine --all-features` passes, and `cargo +nightly-2026-09-05 miri test -p gateway-stt --features test-fixtures miri_` and the same for `gateway-stt-engine` pass in WSL.
- Commit: `Fail the take on speech detector errors`.

</step-5>

<step-6>

### Step 6: Purge the remaining loudness and fallback surface

- Component: Required Silero
- Piece: loudness remnants, last. Each item here became dead or test-only in Step 5.
- Work:
  - `crates/gateway/stt/engine`: `EnergyDetector` and its `lib.rs` export move behind the `test-fixtures` feature. Its remaining users are the scripted runtimes, `crates/gateway/stt/api/src/test_fixtures/segment.rs`, and `crates/gateway/stt/api/tests/it/replay/native_capture/audio.rs`.
  - `crates/gateway/progress/src/lib.rs`: delete `Activity::hub` (`:145`) and its test.
  - `crates/gateway/app/tests/it/realtime_stt/native.rs`: delete `recording_fallbacks` and `LoudnessFallbacks` and their uses in `capture.rs` and `noise.rs` (D1-29). A missing model now fails `native_speech_service_with` at load, which the tests already treat as failure. Update the module docs that mention loudness fallbacks.
  - Keep the native harness's held temporary cache, the orphan-scan exclusion, and `EnginePolicy::is_silence` as the second gate on final decodes and in the whisper backend.
- Verify with the Testing Plan's loudness-fallback-removal checks: the `git grep` returns nothing and `Activity::hub` is gone from `crates/gateway/progress`; `cargo check -p gateway-stt-engine` without features builds no `EnergyDetector`; `cargo nextest run --locked -p gateway --all-features` passes; the native capture (on `local/stt-fixtures/dictation-01.wav`) and noise tests pass on Silero with `PROMPTFORGE_WHISPER_BACKEND=cuda`, `PROMPTFORGE_WHISPER_MODEL` set to the cached `ggml-base.en.bin` and `PROMPTFORGE_WHISPER_FINAL_MODEL` to the cached `ggml-small.en.bin` under `~/.promptforge/models/`, and `PROMPTFORGE_NOISE_CLIPS=local/stt-fixtures/noise` for the noise test.
- Commit: `Remove the loudness detector from production builds`.

</step-6>

<step-7>

### Step 7: Guard released-range settlement

- Component: Take timing guards
- Component order: fourth. Its new finalization tests build takes through the detector API that Step 5 changes, and it precedes the scored captures (Step 9) and the native re-capture (Step 10) in case it changes settlement.
- Piece: released settlement guard (D1-2), built before the timing constants piece because its assertion constrains the same constants.
- Work, all in `crates/gateway/stt/api/src`:
  - `take.rs`: remove `#[cfg(test)]` from the `SPEECH_TAIL_SAMPLES` re-export so `segment.rs` can read it in every build.
  - `segment.rs`: add `const _: () = assert!(SPEECH_TAIL_SAMPLES <= HANGOVER_SAMPLES);` beside the assertion at line 28, importing `SPEECH_TAIL_SAMPLES` from `crate::take`, with a one-line comment: released ranges keep only accepted windows that end inside them.
  - `take/finalization/tests.rs`: a silence-closed range released under the PCM cap, whose last accepted window ends exactly at the range end, keeps its interim text final. A released forced stride whose accepted windows extend past the stride end loses no words from the completed transcript.
  - Only if the forced-stride test fails: add close-kind-aware clipping in `record_outcome` (`take/finalization.rs:271`). Silence closes clip as `record_skipped_segment` does, and forced strides leave the overlap to the successor. Say so in the return, so the dispatching session moves that item from Deferred and Out of Scope into the Decision Record.
- Verify with the Testing Plan's D1-2 checks: set `SPEECH_TAIL_SAMPLES` above `HANGOVER_SAMPLES` once by hand, confirm `cargo build -p gateway-stt` fails, and revert. `cargo nextest run --locked -p gateway-stt --all-features finalization` passes, and Miri for `gateway-stt` passes in WSL.
- Commit: `Guard released-range settlement against a longer speech tail`.

</step-7>

<step-8>

### Step 8: Spell timing constants by name in tests

- Component: Take timing guards
- Piece: timing constants in tests (D1-23), after Step 7.
- Work, tests only, in `crates/gateway/stt/api`:
  - `src/segment/endpoint.rs` tests (lines 256-377): `SENTENCE_END_SILENCE_SAMPLES` replaces `3_200`.
  - `src/take/window/echo.rs` tests: `HANGOVER_SAMPLES` replaces `const HANGOVER: u64 = 1_600` (line 99). Widen `HANGOVER_SAMPLES` (`segment/endpoint.rs:45`) to `pub(crate)` and re-export it through `segment` if needed.
  - `tests/it/replay/repeats.rs:95-96`: `SPEECH_TAIL_SAMPLES` replaces `1_600`, reached through a new `test_fixtures` re-export.
  - Leave append chunk sizes and JSON script data alone, including `"at_ms": 3_200` at `repeats.rs:105`.
- Verify with the Testing Plan's D1-23 retune check: with `SENTENCE_END_SILENCE_SAMPLES` temporarily set to 4,800 samples, the endpoint, echo, and repeat tests pass with no test edits. Revert the constant. Replay goldens are excluded from this check.
- Commit: `Spell speech timing constants by name in tests`.

</step-8>

<step-9>

### Step 9: Pin short echo phrases and trial-remove the phrase veto

- Component: Phrase veto trial
- Component order: fifth. Its keep-or-restore decision rests on scored captures, which must run on the final detector wiring (Steps 4 through 6) and settlement (Step 7).
- Piece: one piece, one step. The echo tests and the veto trial share the D1-1 checks and one commit.
- Work:
  - Add tests in the `crates/gateway/stt/api/src/take/window/echo.rs` tests or `crates/gateway/stt/api/tests/it/replay/repeats.rs` showing that "Bye." and a bare "you" after a sentence are never cut.
  - `crates/gateway/stt/backend-whisper/src/guard.rs`: remove `SILENCE_HALLUCINATIONS` (line 14), its check (line 82), and its tests (around line 348). Keep the confidence veto and loop collapse. The echo cut and `repeats_final` stay unchanged.
  - Run the native noise test and the gateway-level capture on both `local/stt-fixtures/dictation-01.wav` and `local/stt-fixtures/jfk.wav`, scoring each capture with the Project Survey commands.
    - Settings: `PROMPTFORGE_WHISPER_BACKEND=cuda`, `PROMPTFORGE_WHISPER_MODEL` set to the cached `ggml-base.en.bin`, and `PROMPTFORGE_WHISPER_FINAL_MODEL` to the cached `ggml-small.en.bin` under `~/.promptforge/models/`.
    - `PROMPTFORGE_NOISE_CLIPS=local/stt-fixtures/noise` for the noise test.
    - `PROMPTFORGE_CAPTURE_LINES` set to `local/stt-fixtures/dictation-01-lines.json` or `local/stt-fixtures/jfk-lines.json` when scoring.
  - Keep the removal only if all of these hold:
    - The noise test shows no text.
    - dictation-01 keeps final punctuation on at least 10 of 11 lines with at most 1 word error.
    - JFK keeps final punctuation on at least 2 of 4 lines with no word errors.
    - Neither capture shows unsaid text during a pause.
  - These are the tuned Silero results. If a capture shows "please subscribe", "thanks for watching", or "thank you for watching" as unsaid text, or misses any criterion, restore the veto and report the hit and scores in the return, so the dispatching session records them in the Decision Record.
- Verify with the Testing Plan's D1-1 checks, plus `cargo nextest run --locked -p gateway-stt -p gateway-stt-backend-whisper --all-features`.
- Commit: `Remove the silence phrase veto`, holding the echo tests and the removal. If the veto is restored, commit only the echo tests as `Pin short phrases against the echo cut`.

</step-9>

<step-10>

### Step 10: Replace the hand-edited native replay fixture

- Component: Native replay fixture
- Component order: sixth. The capture decodes through the whisper backend and the take pipeline, so it follows every change to them: the detector wiring (Steps 4 through 6), settlement (Step 7), and the text guard (Step 9).
- Piece: one piece, one step.
- Work, all in `crates/gateway/stt/api/tests`:
  - Run the ignored capture in `it/replay/native_capture.rs` on `jfk.wav`, with `PROMPTFORGE_REPLAY_CAPTURE` naming a scratch `<name>.json` outside the repository and `PROMPTFORGE_WHISPER_LIBRARY` set to `local/stt-fixtures/whisper.dll`. Use the model named in the `33ae08d37` commit message, or `local/stt-fixtures/ggml-tiny.en.bin` when it names none.
  - Copy the script and snapshots unchanged to `fixtures/replay/jfk-native-recaptured.json` and `fixtures/replay/jfk-native-recaptured.snapshots.json`. Set `NATIVE_FIXTURE` (`it/replay.rs:27`) to `"jfk-native-recaptured"`. Keep `jfk-native.json` and its baseline section as frozen history.
  - The module docs of `it/replay.rs` and `it/replay/native_capture.rs` state that native fixtures are re-captured under a new name and never hand-edited.
  - Run the replay suite once with `PROMPTFORGE_REPLAY_UPDATE=1` to record the new baseline section and metrics. Never edit existing baseline numbers.
  - This is the last step, so it also runs the exit checks the full-scope verification does not cover. Run Miri for `gateway-stt` and `gateway-stt-engine` in WSL. The native noise test and both scored captures already ran in Step 9, on production code this step does not change. The build, formatter, linter, docs, and full suite run in this step's full-scope verification.
- Verify with the Testing Plan's D1-3 checks: the committed `jfk-native-recaptured*.json` files show no differences from the scratch capture output, `cargo nextest run --locked -p gateway-stt --all-features replay` passes, and `git diff -- crates/gateway/stt/api/tests/fixtures/replay/baseline.json` shows only added lines. Miri passes for both crates.
- Commit: `Re-capture the native JFK replay fixture`.

</step-10>

</execution-plan>
