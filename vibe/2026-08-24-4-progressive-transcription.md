---
name: Progressive transcription pipeline
overview: Committed/tentative transcription with mid-utterance large-model crystallization, domain vocabulary biasing, and the textarea growth fix - six tight commits, each with its own test.
todos:
  - id: c1-textarea-grow
    content: "Commit 1: fix textarea auto-grow (voice-driven text never expands the box)"
    status: completed
  - id: c2-final-callback
    content: "Commit 2: FinalTranscriber per-take completion callback channel"
    status: completed
  - id: c3-committed-wire
    content: "Commit 3: session committed state + interim wire frame {committed, tentative} + bounded interim window"
    status: completed
  - id: c4-stop-tail
    content: "Commit 4: stop path - final_finish returns tail only, final frame = committed + tail"
    status: completed
  - id: c5-client
    content: "Commit 5: client parses committed/tentative, displays concatenation"
    status: completed
  - id: c6-vocabulary
    content: "Commit 6: vocabulary biasing - config field, glossary prompt formatting, worker integration"
    status: completed
isProject: false
---

# Progressive transcription: committed + tentative + vocabulary biasing

Six tight commits. Each is the largest slice one test covers completely. Coder then review-and-fix per commit; Verify on commits 3 and 6 (and whenever review dirties the tree). Follow the rust-rulebook and vibe-rulebook. Append design log entries (design/design-promptforge-wb-1.md, numbered, Choice/Evidence/Cost; last entry is ~79) in the commit that makes each decision.

Direction (adjudicated with the user): build on the existing whisper-rs stack. Do NOT switch to transcribe-cpp (different GGUF format, pre-1.0, single maintainer). Steal the stable-prefix idea, not the crate.

## Commit 1: fix textarea auto-grow

**Bug:** voice-driven text lands in the textarea but the box never grows. murm-ui's `components/input.ts:144-149` `adjustHeight()` only runs on real user keystrokes (its own input listener). Our programmatic `input.value = text` doesn't reach it - we removed the `dispatchEvent` because jsdom's realm rejects synthetic Events in the smoke test. Our own `resizeInput()` in voice.ts sets inline `style.height`, but something still fails in the real app.

**Work:**
- Diagnose precisely in the running app: is `resizeInput()` producing a wrong value (e.g. `scrollHeight` read before layout, `window.innerHeight` oddity in WebView2), or is murm-ui's CSS (`input.css:59` `height: 36px`, `:58` `max-height: var(--mur-input-max-height, 200px)`) overriding?
- The robust fix: dispatch the `input` event so murm-ui's own `adjustHeight` runs (it is the canonical resizer), and drop our duplicate `resizeInput()`. The jsdom realm issue was with `new Event(...)` constructed from the wrong global - fix the smoke test by constructing the event from `window.Event` inside the bundle (the bundle already resolves `Event` from globalThis, which the test copies from jsdom's window - verify this works; if the realm issue persists, keep `resizeInput` but fix whatever it gets wrong).
- Raise the cap: set `--mur-input-max-height` in style.css `:root` to `40vh` so long transcripts are visible.
- Test: jsdom smoke asserts the textarea's inline height grows when a multiline interim arrives.

## Commit 2: FinalTranscriber completion callback

**Goal:** the final worker reports each completed segment's text back to the session, instead of silently accumulating until `final_finish`.

**`transcribe.rs`:**
- `FinalTranscriber` gains a per-take callback: `final_reset(&self, on_segment: std::sync::mpsc::Sender<String>)` - the channel is installed at take start, dropped/replaced on the next reset.
- The final worker, after transcribing a submitted segment, sends the segment text on the channel (best effort: `send` may fail if the session is gone - ignore with a debug log).
- `final_finish` behavior unchanged in this commit.
- Test: unit test - construct engine from the existing test fixtures, `final_reset` with a channel, `final_submit` a fixture segment, assert the channel receives the segment's text; then `final_finish` still returns the full assembled transcript.

## Commit 3: committed state + interim wire protocol

**`voice.rs`:**
- Session gains `committed: Arc<Mutex<String>>` (or a `watch::channel<String>` - coder's call, note in design log).
- The binary-message handler, after each `final_submit`, drains the callback channel (non-blocking `try_recv` loop) and appends to `committed`.
- The interim loop reads `committed` and transcribes only audio after `segmenter.consumed()` (bounded window - no O(n^2) growth on long takes).
- Interim wire frame becomes `{"type":"interim","committed":"...","tentative":"..."}`.
- Test: WS-level test with the existing fixture infrastructure - feed audio, assert interim frames carry both fields and `committed` is append-only across frames.

## Commit 4: stop path - tail-only final finish

**`transcribe.rs`:** `final_finish` returns only the tail segment's text (the audio after the last committed segment), not the full assembled transcript. The accumulated full text is still available internally for the prompt-conditioning chain, but the return value is tail-only.

**`voice.rs` stop path:** final frame text = `committed` (already crystallized) + tail text from `final_finish`. Fallback paths (no final model / final error) keep working: committed + interim-model tail decode.

**Why:** stop latency becomes proportional to the tail, not the whole take. On a long dictation most segments are already crystallized when the user stops.

- Test: end-to-end with fixtures - a take with a silence gap in the middle; assert the final frame equals committed prefix + tail, and that the final worker was not asked to re-transcribe committed segments.

## Commit 5: client committed/tentative

**`ui/src/voice.ts`:**
- `handleVoiceMessage` interim branch: read `committed` and `tentative` strings, display `committed + tentative` in the textarea via `showInterim`.
- Keep the grow-only guard (`text.length >= input.value.length`) as a safety net.
- Final branch unchanged (sets `input.value` to the final text).
- Test: jsdom smoke - feed an interim frame with committed+tentative, assert the textarea shows the concatenation; feed a shorter tentative, assert the text doesn't shrink while committed is unchanged.

## Commit 6: vocabulary biasing

**Goal:** domain terms (MCP, Lua, GGUF, axum, tokio, Boost.Beast, coroutine, etc.) resolve correctly instead of being misheard.

**`config.rs`:** `VoiceConfig` gains `vocabulary: Vec<String>` (default empty). Generated template in `promptforge-wb/src/discover.rs` gets a commented-out example line. `workbench.example.toml` documents it.

**`transcribe.rs`:** format the vocabulary as a glossary prompt - `"Glossary: term1, term2, ..."` (this format measurably outperforms raw keyword lists per the research). Prepend to the existing segment-conditioning prompt on the final worker; pass as `initial_prompt` on the interim worker. Respect whisper's 224-token prompt cap: truncate the glossary to fit (log a warning when truncated).

- Test: unit-test the glossary formatting and token-cap truncation; config parse test for the new field; if a fixture exists with a tricky term, an integration test asserting the biased decode - otherwise unit coverage suffices.

## Verify schedule

- Verify (full workspace: fmt, clippy, test) after commits 3 and 6, plus whenever review-and-fix dirties the tree.
- `npm run build; npm run typecheck; npm test` after commits 1 and 5.

## Rules of engagement

- One commit per numbered step: code + test + docs together.
- Coder subagent per commit, review-and-fix subagent after each, amend fixes into the commit.
- tape.jsonl, ui/node_modules, ui/dist: never stage. Stage by explicit path.
- Windows PowerShell: `;` not `&&`; use working_directory, never cd.
- Design log entries for: the callback channel shape (commit 2), committed-state container choice (3), tail-only finish (4), glossary prompt format and truncation policy (6).


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: Progressive transcription pipeline

## Origin and intent (creator chat, 2026-08-24)

The plan was born mid-session when the user rejected the batch-style transcription that had just shipped. Verbatim:

> "once we have so much text that the beginning is unlikely to be reinterpreted, then I wanna speculatively run the large model ... we can crystallize what the beginning is gonna be, and then as they keep talking, then we crystallize the next segment. ... the end of the text is gonna be volatile, it's gonna keep getting rewritten. But once you go past a certain size, we shouldn't be rewriting the beginning. ... by the time that if the user stops recording After a big input of text, we've already done the big model ... and then the suffix we can just resolve. ... this is how Cursor works. ... I want prior art now."

Requirements (verbatim fragments): "I wanna be able to have accuracy, and I want the user to see the words visibly" and "it doesn't have to be real time. So we can trade that off a little bit" - accuracy over latency. Plus domain vocabulary: "I want to have the capability to give it a vocabulary that's like specific to PromptForge ... like MCP, like it knows what MCP is. So when the user says a word that's ambiguous, then it can resolve it in the context of ... some kind of AI work with PromptForge." The textarea growth bug was folded in from the same message ("the edit box doesn't grow vertically ... There's a bug in the UI").

Decomposition constraint (user, verbatim): "decompose this if possible I want these commits tight."

## Prior art and discarded alternatives

Research (two subagents) mapped the request to the committed+tentative pattern. Candidates (paraphrase):

- transcribe-cpp (Rust): `StreamText { committed, tentative }` with N-agreement stable-prefix commits - cleanest API match.
- WhisperPipe: consensus via Levenshtein similarity; commits stable words mid-utterance; trims the audio buffer so re-transcription stays O(1), not O(n^2).
- Pipecat #5009: tiny whisper for interims + distil-medium for finals, but the large model runs only at speech end, not mid-utterance.
- Google Streaming Deliberation: the second pass is itself a streaming transducer that refines incrementally - the ideal endpoint.

Three options were adjudicated:

- Option A (chosen): build committed/tentative on the existing whisper-rs stack. Rationale (paraphrase): the final worker already transcribes completed segments in the background during recording; the machinery was "half-built - it just doesn't surface the results until stop." Roughly 200 lines of new code on proven infrastructure, and vocabulary biasing comes free via `initial_prompt`.
- Option B (discarded): replace whisper-rs with transcribe-cpp. Rejected because it uses a different GGUF format (existing downloaded models unusable), is pre-1.0 with breaking changes between 0.1 and 0.2, is single-maintainer, and has no merged hotwords. Verdict (paraphrase): switching engines to save ~200 lines would cost a week and add risk for no accuracy gain - it is the same Whisper architecture underneath.
- Option C (partially absorbed): steal `CommitPolicy::StablePrefix` (N-agreement on consecutive hypotheses). The plan as written did not implement N-agreement; crystallization is instead driven by the energy segmenter's silence gaps, which the pipeline already had.

Vocabulary biasing format: `"Glossary: term1, term2, ..."` was chosen over raw keyword lists because benchmarks showed roughly 50% WER reduction (research claim, paraphrase). It is a soft bias, not a guarantee; post-processing regex for critical terms was suggested as a complement but not planned.

## Post-plan design evolution (creator chat, same day)

After the six commits landed, live dictation exposed volatility. The user benchmarked against Cursor and corrected the assistant's claim that Cursor is batch-only, verbatim: "No, you're wrong about that. ... Cursor works exactly how I told you I wanted my program to work. ... once Cursor settles down, it's very stable. Like when, when you're talking, and then if you pause for two seconds, it crystallizes, and then you can keep talking, and it's really good." This drove parameter retuning without a plan ("just do it without a plan"): silence-to-close 700ms -> 2s, interim window 5s -> 15s, interim interval 800ms -> 500ms.

A second bug: on stop, the text shrank to nothing and then returned. Root cause (paraphrase): after the segmenter closes a segment, the interim window becomes trailing silence, whisper returns empty text, and the empty frames blank the display before the final worker crystallizes the segment. The user proposed the fix shape, verbatim: "would keeping 2 strings help? the current string, and the new prefix, and then just substitute it in?" and ordered a design review: "think deeply about the latch, consider the surrounding code and see if you can poke a hole in it." Safety invariant demanded verbatim: "we need to make sure that we are never trying to do 2 final passes at the same time" - once confirmed structurally impossible, "implement the latch." The latch: suppress empty-tentative frames unless committed has grown past the snapshot taken at the last non-empty frame, so the display never blanks between segment close and crystallization.

## Run-chat deviations

Run chat 8989e7ea (coder subagent, commit 3 - committed state + interim wire protocol):

- Container choice: a `Committed { text, segments }` struct behind `Arc<std::Mutex>` (same poisoning posture as the PCM buffer), chosen over a watch channel so `begin_take` clears and installs in one lock; the consumed offset rides an `Arc<AtomicUsize>` because the segmenter stays session-local (design log entry 82).
- Added a send-on-change policy: interim frames are sent when either field changes, so committed-only updates still flow while tentative is empty.
- Test deviation: the planned append-only WS test had to be redesigned because GPU whisper crystallizes both fixture segments within a single interim pass - the first received frame already carried the final committed value, so an assertion waiting for two distinct committed values could never fire.
- Clippy pressure (pedantic lints promoted to errors by `-D warnings`) forced an `expect(too_many_arguments)` and helper extraction to keep `run_session` under the 100-line limit.
- A pre-existing `chat_ws` reconnect-test flake reproduced on the base commit; unrelated to the change.

Run chat 59952dc1: unrelated to this plan (transcript mining / skillgate tooling). No deviations recorded.
