# Generic Realtime STT installed-package acceptance

## Status

Verification round 3 passed the complete automated release suite, rebuilt and silently installed a fresh unsigned package, and passed the installed local-sidecar recovery scenario. Workshop and Gateway stayed live beyond 60 seconds, Workshop survived forced Gateway termination, and a replacement was accepted by changed PID and boot identity while the configured bearer remained stable. The installed replacement passed process-image, health, accepted-bearer, rejected-bearer, Workshop relay, config-proxy, and Realtime checks. Deterministic tests passed for atomic publication to heartbeat, catalog, chat, progress, config proxy, and Realtime, fixed explicit-LAN behavior, same-port and same-key replacement, configured-key edits, and exactly one boundary space between two standalone no-leading dictation takes.

- Automated release gate: passed
- Installed-process gate: passed
- Post-relaunch recovery gate: passed
- Physical microphone and device-error checks: passed by final operator acceptance
- Installed application: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Installed Workshop left open: PID 87536
- Installed Gateway left open: PID 96452 on port 51984
- Signing: not tested; release signing remains release-CI-only
- Commit created: no

## Step 32 deterministic arbitrary-duration acceptance

### Acceptance boundary

- Date: 2026-09-08
- Scope: deterministic production-session, mounted Gateway, Workshop relay, and Workshop UI verification
- Physical microphone acceptance: not performed and not claimed
- Commit created: no

### One-hour one-take proof

`one_take_runs_for_an_hour_with_bounded_absolute_ownership` drives the production Realtime `Session`, interim scheduler, final pipeline, rolling PCM compaction, overlap reconciliation, mailbox, commit, and completion with 360 ten-second strides. It uses six rotated append sizes of 1, 23,999, 72,000, 17, 47,983, and 96,000 input samples, no sleep, and no hour-sized audio allocation. Every received 16 kHz window contains absolute second markers derived from the actual 24 kHz PCM. The decoder derives coverage and transcript ranges from those markers and rejects wrong, stale, duplicated, reordered, or incorrectly compacted samples.

- Logical ownership: one provisional item ID remains unchanged for all 360 strides; one `UncommittedInput` and `Take` are promoted by one commit
- Completion: one completed result for that same item and no second terminal
- Usage: exactly 86,400,000 input samples at 24 kHz and exactly `3,600.0` completion seconds
- Absolute coverage: finalized coverage plus the sole unresolved forced range remains contiguous from sample zero through 57,600,000 at 16 kHz, then completion resolves the full range
- Final windows: 360 accurate decodes; the first owns 10 seconds and every later window owns at most 18 seconds through the fixed 8-second overlap
- Retained ownership: allocated resident, queued, and actively decoding PCM remains under the exact 480,000-sample, 30-second budget
- Bounded state: the hour proof retains at most one queued final, one pending final outcome, and two accepted hypotheses at each settled stride; the independent blocked-worker scenario exercises two queued finals and the exact retained-budget refusal
- Hypotheses: 360 production interim revisions remain complete replacement snapshots with stable absolute audio spans while finalized audio compacts
- Final transcript: the authoritative result contains each of the 3,600 timeline tokens exactly once
- Mounted Gateway: the production WebSocket route receives the same 360 marked strides, retains one item ID, accepts one commit, emits one completion, and reports exactly 3,600 seconds
- Natural pauses: 8,209 decoded-speech and silence cycles settle online with zero retained outcome history, while a partial short-final skip retains at most one range until it consumes its accepted hypothesis
- Transient PCM: source resampler capacity is charged before destination growth or copying; the exact combined source, destination, and growth peak succeeds and one sample less of budget fails before mutation

The focused boundary suite also covers stop before, exactly on, and after a forced cut; a natural boundary following a forced cut; repeated phrases; punctuation and case normalization; fail-closed missing alignment; clear; session and worker cancellation; and slower-than-capture overload. These are deterministic protocol and ownership claims, not claims about physical microphone capture duration.

### Throughput overload behavior

The mounted Gateway test blocks an accurate decoder, admits the next ten-second stride while the first decode remains owned, then rejects the following transient source-plus-destination allocation with decoded code `too_much_unfinalized_audio` and commits the still-valid input through the same mounted socket. The Workshop relay passes 360 reused bounded chunks opaquely and preserves one item, one commit, and one 3,600-second completion. The pure `TakeRegistry` and production `setupStt` tests stream 360 bounded appends while retaining only the latest active append correlation; a stale retired correlation cannot mutate the take, and duplicate overload delivery causes one capture stop and one commit with no clear or rollback. Other decoded server errors continue through the existing rollback path.

### Step 32 automated results

- Gateway: full package tests passed, including 96 integration tests with 5 ignored native cases; the mounted hour and slower-than-capture Realtime tests passed
- Gateway STT: default and `test-fixtures` suites each passed 64 tests with 2 ignored native cases; the one-hour proof passed in 8.8 seconds
- Workshop relay: 10 passed
- Workshop UI: typecheck, production build, layer gate, 87-test suite, and dedicated leak check passed
- Gateway Config UI: typecheck, production build, layer gate, and 129-test suite passed
- Miri: engine filter passed 3 tests; Gateway STT filter passed 13 tests
- Architecture and API: architecture self-tests, exact feature API snapshot, source-module ceilings, integration test manifest, crate dependency graph, and Rust architecture harness passed
- Repository hygiene: `cargo fmt --all --check` and warnings-denied all-target all-feature workspace Clippy passed
- Guides: source generation passed twice and the second generation produced byte-identical outputs

## Step 42 verification round 3 at HEAD 5988a6c0

### Run boundary

- Current commit: `5988a6c023f130756a9850e182f3f1a84619dd8c` (`[WIP] Step 42: Run every release gate and repeat acceptance`)
- Shell: Windows PowerShell `5.1.26100.9278`
- Automated suite started: `2026-09-07T15:21:45.1477842Z`
- Automated suite finished: `2026-09-07T15:34:01.5942626Z`
- Initial worktree: clean
- Commit created: no

### Complete automated release suite

Every Step 42 command ran independently and exited with code 0:

- Rust format, all-target all-feature lint, workspace tests, all-feature documentation tests, warning-denied documentation generation, dependency-policy audit, Gateway build, Workshop build, and featureless Gateway check: passed
- Native Whisper equivalence: 5 passed, 0 failed
- Miri engine target: 2 selected tests passed, 0 failed
- Miri protocol target: 11 selected tests passed, 0 failed
- STT architecture script: passed with acyclic crates and exact public-root counts `6, 7, 2, 6`
- STT architecture integration harness: 16 passed, 0 failed
- Workshop UI type and layer gate: passed
- Workshop UI production build: passed
- Workshop UI suite: 69 passed, 0 failed
- Gateway config UI type gate: passed
- Gateway config UI production build: passed
- Gateway config UI suite: 128 passed, 0 failed
- User-guide generation: passed
- mdBook build: passed
- Explicit generated-guide cleanliness diff: passed

### Deterministic replacement and dictation gates

- Gateway boot identity tests: 18 passed, 0 failed
- Workshop supervision tests: 16 passed, 0 failed; coverage includes liveness beyond 60 seconds, bounded relaunch, same-port and same-key replacement with a new PID, PID or boot-identity recognition, process-image and bearer validation, configured-key propagation, and unmanaged explicit LAN
- Explicit LAN focused gate: 1 passed, 0 failed
- Atomic Gateway snapshot tests: 2 passed, 0 failed
- Heartbeat and model-catalog replacement gate: 1 passed, 0 failed
- Progress replacement gate: 1 passed, 0 failed
- Config-origin and proxy replacement gate: 1 passed, 0 failed
- Live chat session replacement gate: 1 passed, 0 failed
- Browser Realtime retry through the unchanged Workshop process: 1 passed, 0 failed
- Agent dictation gate: all assertions passed, including two standalone no-leading takes composing as `First test alpha Second test beta` with exactly one boundary space and no duplicate separator when either side already supplies one

### Generated-document cleanliness

Guide regeneration, mdBook compilation, and the explicit cleanliness diff passed. SHA-256 identities after regeneration:

- `guide/src/SUMMARY.md`: `4031AACD9459ED213C3E5D41466993691FD8B2DA07DEC9D090D90E8493F99FFC`
- `guide/src/gateway/index.md`: `09E9807249611001CA6CAF2A1A210BF64E2B843C4C6B6A8C7284068F6E44B2D2`
- `guide/src/workshop/index.md`: `4BC7756A0D6D66807061BD747C72618096B24ACC5031F536F12B4019C53F4226`
- `guide/src/language/index.md`: `41E9E4458BC1ED9F969F0DB7E13C24A3D94A4E88253D88239E6AA3F40F631AFD`
- `guide/src/agent/index.md`: `1C66A4A2EF1AF16AE38668F2D0910124F20520E714610EA8B150C282ECAC623E`
- `guide/promptforge-gateway-guide.md`: `5BF95CD9776A87982E13D9C6E7DA09F7BED2375292E75919F963BF59F497BCDE`
- `guide/promptforge-workshop-guide.md`: `F45DB5FBAE9B56CB4415218CBA2B8D12EEAB39B45DEF10922349C369E2921DF0`
- `guide/promptforge-language-guide.md`: `3CDF6E562EF45AC8873703C81701834650CAC7E8A8459839E96466D03AF16DFA`
- `guide/promptforge-agent-guide.md`: `3B70D4DE22FF4077BC31D9E484BC8672DDF256413795B6BD8708774DB29464F9`

### Fresh unsigned package and identities

- Stable locked Gateway release build: passed
- Target-suffixed sidecar staging: passed
- Release, staged, and installed Gateway SHA-256: `C60DFDEBC6E45EEE82AF6952FF81CF1B0A1F0B92ADC8EA1BFE68F82105E8C06B`
- Tauri CLI locked install gate: passed with version 2.11.4 already installed
- Fresh unsigned NSIS package build: passed
- Final installer SHA-256: `5ABFB2436BC18FCD8DD9EF0D0923F414AB47E0D20661978C11A8520305AA21ED`
- Silent install exit code: 0
- Installed Workshop SHA-256: `6A25AD42F771CE07C1BD7D70365C0436D5CF6CEC91E83D506D97B98C47548C4B`
- Installed Workshop identity: product `PromptForge`, version `0.2.0`, unsigned, expected installed path
- The installed and bundle-stage Workshop images had equal size and matching product identity but different PE hashes after NSIS extraction
- Protected release configuration cleanliness: passed
- Signing: not tested; release signing remains release-CI-only

The first round 3 package observation successfully built and installed the package, then stopped on a verifier-added Workshop byte-equality assertion. That assertion was not a release requirement and was invalid for the observed NSIS image transformation. The failure remains recorded. The corrected identity gate checks installed path, product name, product version, image size, and unsigned status, while the Gateway sidecar retains byte-for-byte release, staging, and installed equivalence. The corrected package and installation phase was then repeated from the release build and passed.

### Installed local-sidecar recovery

- Installed Workshop launched at `2026-09-07T15:37:39.9988414Z`, PID 87536, loopback port 55805
- Initial installed Gateway: PID 100512, port 55799, installed sibling image, health 200, bearer-authenticated model catalog 200
- Credential present: yes; value not recorded
- Installed-pair liveness interval: 65.061 seconds
- Workshop and Gateway remained running beyond 60 seconds: yes
- Initial Gateway force-terminated at `2026-09-07T15:38:47.5683322Z`
- Workshop remained open across termination with PID 87536: yes
- Replacement observed after 3.299 seconds
- Replacement Gateway: PID 96452, port 51984, boot identity `2026-09-07T15:38:50.6084321Z`
- Replacement identity changed by PID and boot identity: yes
- Port changed in this observation: yes; deterministic coverage permits OS port reuse
- Configured bearer remained stable: yes; deterministic coverage also proves atomic propagation of a configured edit
- Replacement image path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Replacement health: 200
- Replacement bearer-authenticated model catalog: 200
- Replacement invalid-bearer probe: 401
- Workshop model relay through the replacement: 200
- Workshop config proxy through the replacement: 200
- Workshop published replacement origin: `http://127.0.0.1:51984`
- Browser Realtime through unchanged Workshop: open, first frame `session.created`
- No bearer value was written to the evidence

### Final handoff boundary

- Installed Workshop PID 87536 remains open
- Installed Gateway PID 96452 remains open on port 51984
- Final operator acceptance observed at approximately `2026-09-07T15:41Z`
- Operator followed the requested sequential-take, active-take cancellation, delayed-result suppression, microphone-access denial, access restoration, recovery dictation, and chat-turn checks
- Operator verdict: `wow... fucking brilliant :) works great`
- Physical microphone, device-error recovery, and model-turn checks: passed
- All automated, package, installation, identity, supervision, deterministic consumer-recovery, live replacement, and fixed-LAN gates passed

## Step 42 verification round 2 at HEAD 2d2ee4de

### Run boundary

- Current commit: `2d2ee4dede815e9e19a294285fb0fafd8e9c530b` (`[WIP] Step 42: Run every release gate and repeat acceptance`)
- Shell: Windows PowerShell `5.1.26100.9278`
- Automated suite started: `2026-09-07T14:51:49.7303202Z`
- Automated suite finished: `2026-09-07T15:00:50.9390825Z`
- Initial worktree: clean
- Commit created: no

### Independently executed automated commands

Every listed command ran as its own process invocation and exited with code 0.

- `cargo fmt --all --check`: passed
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed
- `cargo test --workspace`: passed; every selected workspace target reported zero failed tests
- `cargo test --workspace --all-features --doc`: passed; every selected documentation target reported zero failed tests
- `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features`: passed
- `cargo deny check`: passed
- `cargo build -p gateway`: passed
- `cargo build -p workshop`: passed
- `cargo check -p gateway --no-default-features`: passed
- `$fixture=(Resolve-Path 'local\stt-fixtures').Path; $env:PATH="$fixture;$env:PATH"; $env:PROMPTFORGE_WHISPER_LIBRARY=(Resolve-Path 'local\stt-fixtures\whisper.dll').Path; cargo test -p gateway-stt-backend-whisper --test native_whisper -- --ignored`: passed, 5 passed and 0 failed
- `cargo +nightly-2026-09-05 miri test -p gateway-stt-engine --features test-fixtures miri_`: passed, 2 selected tests passed and 0 failed
- `cargo +nightly-2026-09-05 miri test -p gateway-stt --features test-fixtures miri_`: passed, 11 selected tests passed and 0 failed
- `node tools/check-stt-architecture.mjs`: passed
- `cargo test -p gateway-stt --test it architecture`: passed, 16 passed and 0 failed
- Workshop UI `npm run typecheck`: passed; `check-layers: ok`
- Workshop UI `npm run build`: passed; emitted `dist/app.js` at 2.5 MiB and `dist/app.css` at 166.0 KiB
- Workshop UI `npm test`: passed, 69 passed and 0 failed, cancelled, or skipped
- Gateway config UI `npm run typecheck`: passed
- Gateway config UI `npm run build`: passed; emitted `dist/app.js` at 291.4 KiB and `dist/app.css` at 37.7 KiB
- Gateway config UI `npm test`: passed, 128 passed and 0 failed, cancelled, or skipped; `check-layers: ok`
- `cargo run -p build-user-guide`: passed
- `mdbook build guide`: passed
- `git diff --exit-code -- guide/src/SUMMARY.md guide/src/gateway/index.md guide/src/workshop/index.md guide/src/language/index.md guide/src/agent/index.md guide/promptforge-gateway-guide.md guide/promptforge-workshop-guide.md guide/promptforge-language-guide.md guide/promptforge-agent-guide.md`: passed

### Native equivalence and architecture ratchets

- Native Whisper equivalence: 5 passed, 0 failed, covering the fixed JFK transcript, conditioning, job independence, absent-final classification, and progress terminals
- `gateway-stt`: acyclic, 6 public roots
- `gateway-stt-engine`: acyclic, 7 public roots
- `gateway-stt-backend-whisper`: acyclic, 2 public roots
- `gateway-whisper-ffi`: acyclic, 6 public roots
- The final architecture harness passed all 16 exact-dependency, ceiling, migration, isolation, generation, and seam-removal tests

### Deterministic sidecar replacement coverage

The original deterministic gates and the replacement-identity correction gates below all passed:

- `cargo fmt --all --check`: passed after the correction
- `cargo clippy -p workshop -p workshop-server --all-targets --all-features -- -D warnings`: passed
- `cargo test -p gateway boot::tests::`: 18 passed, 0 failed; covered first-run key generation, existing-config discovery without generation, and refusal to overwrite an existing configured key
- `cargo test -p workshop gateway::tests`: 16 passed, 0 failed; covered more than 60 seconds of supervision, bounded relaunch retries, atomic configured-key edit propagation with an OS-reused port, same-port and same-key publication for a new PID, PID or boot-identity replacement detection, full launch validation, rejected bearer handling without disclosure, and fixed unmanaged explicit-LAN behavior
- `cargo test -p workshop-server gateway_binding::tests`: 2 passed, 0 failed; covered one-snapshot endpoint and credential replacement plus invalid-file rejection before publication
- `cargo test -p workshop-server a_replaced_endpoint_wakes_the_heartbeat_and_refreshes_with_its_new_key`: 1 passed, 0 failed; covered health and model-catalog recovery
- `cargo test -p workshop-server an_endpoint_replacement_moves_the_progress_subscription_immediately`: 1 passed, 0 failed; covered progress recovery
- `cargo test -p workshop-server origin_and_config_proxy_follow_one_replacement_snapshot`: 1 passed, 0 failed; covered atomic origin and config-proxy recovery
- `cargo test -p workshop-server --test it a_live_chat_session_restarts_on_the_replacement_port_and_key`: 1 passed, 0 failed; covered chat recovery
- `cargo test -p workshop-server --test it browser_realtime_retry_reaches_the_new_port_and_key_without_workshop_reload`: 1 passed, 0 failed; covered a browser Realtime retry through the unchanged Workshop process

### Generated-document cleanliness

Regeneration and the explicit diff command passed. The post-regeneration SHA-256 identities were:

- `guide/src/SUMMARY.md`: `4031AACD9459ED213C3E5D41466993691FD8B2DA07DEC9D090D90E8493F99FFC`
- `guide/src/gateway/index.md`: `09E9807249611001CA6CAF2A1A210BF64E2B843C4C6B6A8C7284068F6E44B2D2`
- `guide/src/workshop/index.md`: `4BC7756A0D6D66807061BD747C72618096B24ACC5031F536F12B4019C53F4226`
- `guide/src/language/index.md`: `41E9E4458BC1ED9F969F0DB7E13C24A3D94A4E88253D88239E6AA3F40F631AFD`
- `guide/src/agent/index.md`: `1C66A4A2EF1AF16AE38668F2D0910124F20520E714610EA8B150C282ECAC623E`
- `guide/promptforge-gateway-guide.md`: `5BF95CD9776A87982E13D9C6E7DA09F7BED2375292E75919F963BF59F497BCDE`
- `guide/promptforge-workshop-guide.md`: `F45DB5FBAE9B56CB4415218CBA2B8D12EEAB39B45DEF10922349C369E2921DF0`
- `guide/promptforge-language-guide.md`: `3CDF6E562EF45AC8873703C81701834650CAC7E8A8459839E96466D03AF16DFA`
- `guide/promptforge-agent-guide.md`: `3B70D4DE22FF4077BC31D9E484BC8672DDF256413795B6BD8708774DB29464F9`

### Fresh unsigned NSIS package

Packaging began only after the automated suite and deterministic replacement gates passed.

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
  - Result: passed
  - Release Gateway: `C:\Users\Vinnie\cursor\promptforge\target\release\promptforge-gateway.exe`
  - Last modified: `2026-09-07T13:53:06.4583106Z`
  - Size: 13,405,696 bytes
  - SHA-256: `C60DFDEBC6E45EEE82AF6952FF81CF1B0A1F0B92ADC8EA1BFE68F82105E8C06B`
- Command: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
  - Result: passed
  - Staged Gateway size: 13,405,696 bytes
  - Staged Gateway SHA-256: `C60DFDEBC6E45EEE82AF6952FF81CF1B0A1F0B92ADC8EA1BFE68F82105E8C06B`
  - Release and staged hashes matched
- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
  - Result: passed
  - Installed Tauri CLI version remained `2.11.4`
- Initial package invocation at `2026-09-07T15:02:37.7643469Z`:
  - Result: failed before compilation with exit code 2 because PowerShell stripped the inline JSON key quotes
  - Preserved failure: Tauri reported `{bundle:{createUpdaterArtifacts:false}}` was invalid JSON
- Corrected exact PowerShell 5.1 command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo --% tauri build --bundles nsis --config {\"bundle\":{\"createUpdaterArtifacts\":false}}`
  - Result: passed, exit code 0
  - Build started: `2026-09-07T15:02:52.6758669Z`
  - Build finished: `2026-09-07T15:04:11.9473657Z`
  - Protected release configuration remained unchanged
  - `bundle.createUpdaterArtifacts=false` was supplied only through the command line
- Fresh installer:
  - Path: `C:\Users\Vinnie\cursor\promptforge\target\release\bundle\nsis\PromptForge_0.2.0_x64-setup.exe`
  - Created: `2026-09-07T15:03:51.3220507Z`
  - Last modified: `2026-09-07T15:04:11.8150990Z`
  - Size: 12,034,299 bytes
  - SHA-256: `F4BEBF02DBDD6E2B61C2769674EBC8FDCBA9A1A58E00E631C4FA776D7C8F1D0F`
  - Previous installer SHA-256: `194D2D6D86C6E552E12E1E7D96E5469914FE11E588B123E6313833E0C49A9F78`
  - Freshness: creation and modification followed the successful package start, and the SHA-256 changed
  - Signing: not tested; release signing remains release-CI-only

### Installation and installed identities

- Installed PromptForge processes observed before installation: 0
- Silent installer start: `2026-09-07T15:04:12.2545905Z`
- Silent installer finish: `2026-09-07T15:04:15.6907930Z`
- Installer exit code: 0
- Installed Workshop:
  - Path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
  - Last modified: `2026-09-07T15:03:50Z`
  - Size: 24,229,376 bytes
  - SHA-256: `1238646F70A7B84CBEBD12523925022C3215315B51F487258C484F44C6AAEE86`
- Installed Gateway:
  - Path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
  - Last modified: `2026-09-07T13:53:06Z`
  - Size: 13,405,696 bytes
  - SHA-256: `C60DFDEBC6E45EEE82AF6952FF81CF1B0A1F0B92ADC8EA1BFE68F82105E8C06B`
  - Installed, staged, and release Gateway hashes matched exactly

### Installed local-sidecar observation

- Installed Workshop launched: `2026-09-07T15:04:16.1579831Z`
- Workshop PID: 55388
- Workshop path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Workshop loopback port: 57155
- Initial installed Gateway PID: 92432
- Initial installed Gateway port: 57150
- Initial installed Gateway path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Initial Gateway key present: yes; the value was not recorded
- Initial Gateway health status: 200
- Initial Gateway model-catalog status: 200
- Installed-pair liveness interval: 65.060 seconds
- Workshop remained running beyond 60 seconds: yes
- Gateway remained running beyond 60 seconds: yes

The initial Gateway was force-terminated at `2026-09-07T15:05:23.2482189Z` while Workshop PID 55388 remained running. Gateway logging and the live connection file showed an installed sibling replacement starting at `2026-09-07T15:05:26.7613168Z`, with PID 35008 and port 60892. The first observer timed out because it incorrectly required PID, port, and key all to change. Replacement requires a new PID or boot identity, while an OS-assigned port may be reused and an unchanged configuration preserves its long-term credential.

A second direct observation repeated the forced termination:

- Gateway before termination: PID 35008 on port 60892
- Forced termination: `2026-09-07T15:08:24.5934571Z`
- Workshop PID 55388 stayed running: yes
- Replacement Gateway started: `2026-09-07T15:08:27.1643234Z`
- Replacement observed after approximately 2.571 seconds
- Replacement Gateway PID: 59984
- Replacement Gateway port: 61013
- Replacement Gateway path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- PID changed: yes
- Port changed: yes
- Bearer key remained present: yes
- Bearer key changed: no, as expected for the unchanged Gateway configuration
- Bearer key value: not recorded
- Actual installed health, model-catalog, chat, progress, config-proxy, and Realtime recovery probes after relaunch: not run because the observer applied the false changed-key precondition
- Release verdict: incomplete; the replacement process identity passed, but installed recovery and remaining physical acceptance still require observation

### Final handoff boundary

- Installed Workshop PID 55388 remained open at `2026-09-07T15:10Z`
- Installed Gateway PID 59984 remained open on port 61013 at `2026-09-07T15:10Z`
- No microphone, second-take, Clear, cancellation, permission-denial, unavailable-device, or model-turn scenario was physically performed in this verification round
- Signing was not tested; release signing remains release-CI-only

## Final topology and documentation evidence

This section records the Step 40 architecture result. It does not replace or extend the installed-microphone verdict above.

### Debt before and after

- Temporary workspace dependency exceptions: 1 before, 0 after
- Migration-target exceptions: 6 before, 0 after
- Forbidden `gateway-stt -> workshop-server` edges: 1 before, 0 after
- STT source modules above 500 physical lines: 3 before, 0 after
- Largest STT source module: 712 lines before, 481 after
- Effective public-root policy: allowances `9, 7, 2, 6` before; exact counts `6, 7, 2, 6` after
- STT production-library module cycles: 0 after
- Legacy `/stt`, `/stt/capability`, Workshop status/header, and Workshop STT dependency exceptions: 0 after

The engine's 667-line scripted fixture was split into a 362-line fixture and a 304-line test module without changing its 22 unit, 6 contract, 8 startup-cleanup, or documentation test results.

### Final gates

- `node --test tools/check-stt-architecture.test.mjs`: passed, 13 tests
- `node tools/check-stt-architecture.mjs`: passed; all four STT crates acyclic with exact public roots `6, 7, 2, 6`
- `cargo test -p gateway-stt --test it architecture`: passed, 15 tests
- `cargo fmt --all --check`: passed
- `cargo run -p build-user-guide`: passed; all nine generated artifacts had identical SHA-256 values on the clean second run
- `$env:RUSTUP_TOOLCHAIN='stable'; cargo install mdbook --version 0.4.44 --locked`: passed
- `mdbook build guide`: passed

### Final documentation and rules audit

- Added the final architecture design covering ownership, exact dependencies, public counts, Realtime wire policy, bounds, profile replacement, Workshop relay behavior, CI gates, and debt results.
- Updated Gateway, config, Workshop server, and source-guide documentation to remove the retired custom routes and describe `/v1/realtime`.
- Regenerated every guide index and all four single-file exports through `build-user-guide`.
- Corrected the root build prerequisite because a Gateway-only build no longer includes Workshop UI tooling.
- Corrected Workshop's error rule because Workshop no longer provisions STT.
- Audited `gateway-stt`, `gateway-stt-engine`, `gateway-stt-backend-whisper`, `gateway`, and `shared-loopback` rules; their final constraints remain concrete and correct, so they were unchanged.

## Latest installed preparation from HEAD 2d1ecca8

### Source and prior process boundary

- Current HEAD: `2d1ecca839634034d5b70901229d9012e74a18a0`
- Current commit: `2d1ecca8` (`Reconcile explicitly skipped final ranges`)
- Installed Workshop or Gateway processes observed before rebuild: 0
- Installed Workshop or Gateway processes stopped: 0
- Installed Workshop or Gateway processes remaining before rebuild: 0
- Installed Workshop or Gateway processes observed immediately before installation: 0

### Release Gateway

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
- Result: passed
- Summary: release profile finished in 21.80 seconds with 9 `gateway-stt` warnings
- Build started: `2026-09-07T11:38:03.1101939Z`
- Build finished: `2026-09-07T11:38:25.0377360Z`
- Artifact: `target/release/promptforge-gateway.exe`
- Last modified: `2026-09-07T11:38:24.5038009Z`
- Size: 14,536,192 bytes
- SHA-256: `2745D151F7ADD0368308D2029976A11D4BAF38ECBA262C0AC27F57E83D67F74B`

### Target-suffixed sidecar

- Command: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
- Artifact: `crates/workshop/binaries/promptforge-gateway-x86_64-pc-windows-msvc.exe`
- Last modified: `2026-09-07T11:38:24.5038009Z`
- Size: 14,536,192 bytes
- SHA-256: `2745D151F7ADD0368308D2029976A11D4BAF38ECBA262C0AC27F57E83D67F74B`
- Verification: source and staged SHA-256 hashes matched at `2026-09-07T11:38:39.6850080Z`

### Packaging tool

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
- Result: passed
- Installed version: `tauri-cli 2.11.4`
- Detail: Cargo reported that the same version was already installed
- Verified: `2026-09-07T11:38:39.6299436Z`

### Fresh unsigned local NSIS package

- Exact successful PowerShell command: `cargo --% tauri build --bundles nsis --config {\"bundle\":{\"createUpdaterArtifacts\":false}}`
- Working directory: `crates/workshop`
- Result: passed
- Build started: `2026-09-07T11:38:45.9576171Z`
- Build finished: `2026-09-07T11:39:54.3354945Z`
- Workshop release profile finished in 47.06 seconds
- Installer: `target/release/bundle/nsis/PromptForge_0.2.0_x64-setup.exe`
- Installer created: `2026-09-07T11:39:35.7152137Z`
- Installer last modified: `2026-09-07T11:39:54.2055172Z`
- Installer size: 12,393,556 bytes
- Installer SHA-256: `CE476DE44A6F7E0897765ED45AA6E988702826FC9F4B7083A155DBE90E90F028`
- Previous installer SHA-256: `DD2A21369B0834084F26D22ADAE92896431574506C607749F12FFC546ACB78D7`
- Freshness proof: the installer creation and modification timestamps follow the successful build start, and its hash differs from the previous installer
- Override scope: `bundle.createUpdaterArtifacts=false` was supplied only through the Tauri command line
- Protected release configuration: `crates/workshop/tauri.conf.json` and `.github/workflows/release-workshop.yml` have no diff
- Signing: not tested

The adjacent `PromptForge_0.2.0_x64-setup.exe.sig` remains stale from `2026-09-06T02:42:54.1030502Z` and is excluded from this build's evidence.

### Silent installation and installed identities

- Install result: passed
- Installer exit code: 0
- Install started: `2026-09-07T11:40:09.9664232Z`
- Install finished: `2026-09-07T11:40:13.3574707Z`
- Workshop path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Workshop file version: `0.2.0`
- Workshop product version: `0.2.0`
- Workshop last modified: `2026-09-07T11:39:34Z`
- Workshop size: 24,290,816 bytes
- Workshop SHA-256: `36AA10231DA4177C859494B2FD4A116C68EEA12B3BA7C704AB788D16AB6F532C`
- Build-tree Workshop size: 24,290,816 bytes
- Build-tree Workshop SHA-256: `41A5252E66179E48B76C9CED552B730EFF644F18788F87D8D4C6539EBEF1A867`
- Workshop comparison: both identities are recorded without claiming equality because the Tauri log records NSIS bundle-information patching during packaging
- Gateway sibling path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Gateway sibling last modified: `2026-09-07T11:38:24Z`
- Gateway sibling size: 14,536,192 bytes
- Gateway sibling SHA-256: `2745D151F7ADD0368308D2029976A11D4BAF38ECBA262C0AC27F57E83D67F74B`
- Gateway verification: installed, staged, and release SHA-256 hashes match
- Identity verification observed: `2026-09-07T11:40:29.6211886Z`

### Installed application launch

- Launched: `2026-09-07T11:40:35.8875527Z`
- Readiness-window observation: `2026-09-07T11:41:06.4335573Z`
- Workshop process ID: 68872
- Gateway process ID: 91128
- Both process paths resolve under `C:\Users\Vinnie\AppData\Local\PromptForge`
- Both processes remained running at `2026-09-07T11:41:37.3425936Z`
- No physical microphone, model-menu, or model-turn checklist item was observed during automated preparation

## Prior installed preparation after whole-window scheduler repair

### Source and prior process boundary

- Current HEAD: `006ba06d945ec0bfacbb0a0270f65d2706eb20db`
- Current commit: `006ba06d` (`Schedule and rebase whole-window hypotheses`)
- Installed Workshop or Gateway processes observed before rebuild: 0
- Installed Workshop or Gateway processes stopped: 0
- Installed Workshop or Gateway processes remaining before rebuild: 0
- Process boundary observed: `2026-09-07T10:42:20.0755391Z`

### Release Gateway

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
- Result: passed
- Summary: release profile finished in 21.49 seconds with 9 `gateway-stt` warnings
- Build started: `2026-09-07T10:42:19.6689474Z`
- Build finished: `2026-09-07T10:42:41.2751764Z`
- Artifact: `target/release/promptforge-gateway.exe`
- Last modified: `2026-09-07T10:42:40.7268324Z`
- Size: 14,524,928 bytes
- SHA-256: `E3D4DD8694423F69DBE1A828BD2915C04E1E6963E2510867F6FC4A1E170C0FA4`

### Target-suffixed sidecar

- Command: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
- Artifact: `crates/workshop/binaries/promptforge-gateway-x86_64-pc-windows-msvc.exe`
- Last modified: `2026-09-07T10:42:40.7268324Z`
- Size: 14,524,928 bytes
- SHA-256: `E3D4DD8694423F69DBE1A828BD2915C04E1E6963E2510867F6FC4A1E170C0FA4`
- Verification: source and staged SHA-256 hashes matched at `2026-09-07T10:42:47.8474773Z`

### Packaging tool

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
- Result: passed
- Installed version: `tauri-cli 2.11.4`
- Detail: Cargo reported that the same version was already installed
- Verified: `2026-09-07T10:42:47.7978686Z`

### Fresh unsigned local NSIS package

- Exact successful PowerShell command: `cargo --% tauri build --bundles nsis --config {\"bundle\":{\"createUpdaterArtifacts\":false}}`
- Working directory: `crates/workshop`
- Result: passed
- Build started: `2026-09-07T10:42:55.4726299Z`
- Build finished: `2026-09-07T10:44:25.3444665Z`
- Workshop release profile finished in 59.98 seconds
- Installer: `target/release/bundle/nsis/PromptForge_0.2.0_x64-setup.exe`
- Installer created: `2026-09-07T10:43:58.3597272Z`
- Installer last modified: `2026-09-07T10:44:16.9471184Z`
- Installer size: 12,507,285 bytes
- Installer SHA-256: `DD2A21369B0834084F26D22ADAE92896431574506C607749F12FFC546ACB78D7`
- Previous installer SHA-256: `CD114A03A98F5E9F3354DC889998742E01EB3C221DB0CA61F0AA835DBDED885D`
- Freshness proof: the installer creation and modification timestamps follow the successful build start, and its hash differs from the previous installer
- Override scope: `bundle.createUpdaterArtifacts=false` was supplied only through the Tauri command line
- Protected release configuration: `crates/workshop/tauri.conf.json` and `.github/workflows/release-workshop.yml` have no diff
- Signing: not tested

The adjacent `PromptForge_0.2.0_x64-setup.exe.sig` remains stale from `2026-09-06T02:42:54.1030502Z` and is excluded from this build's evidence.

### Silent installation and installed identities

- Install result: passed
- Installer exit code: 0
- Install started: `2026-09-07T10:44:29.7572459Z`
- Install finished: `2026-09-07T10:44:33.1346052Z`
- Workshop path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Workshop version output: `promptforge-workshop 0.2.0`
- Workshop file version: `0.2.0`
- Workshop product version: `0.2.0`
- Workshop last modified: `2026-09-07T10:43:56Z`
- Workshop size: 24,881,664 bytes
- Workshop SHA-256: `3D95568DECE542DC0D56404FBFFB49567EAACED58E1A8CB11B5836572D33B8B6`
- Build-tree Workshop size: 24,881,664 bytes
- Build-tree Workshop SHA-256: `6779FC9020AC6CF48299E16F77EF1EBCE3533056185295F9DF13882E20FA618B`
- Workshop comparison: both identities are recorded without claiming equality because the Tauri log records NSIS bundle-information patching during packaging
- Gateway sibling path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Gateway sibling last modified: `2026-09-07T10:42:40Z`
- Gateway sibling size: 14,524,928 bytes
- Gateway sibling SHA-256: `E3D4DD8694423F69DBE1A828BD2915C04E1E6963E2510867F6FC4A1E170C0FA4`
- Gateway verification: installed, staged, and release SHA-256 hashes match
- Identity verification observed: `2026-09-07T10:44:43.0748367Z`

### Installed application launch

- Launched: `2026-09-07T10:44:51.0854084Z`
- Readiness-window observation: `2026-09-07T10:45:11.1809954Z`
- Workshop process ID: 95492
- Gateway process ID: 77192
- Both process paths resolve under `C:\Users\Vinnie\AppData\Local\PromptForge`
- Both processes remained running at `2026-09-07T10:45:38.3200361Z`
- No physical microphone, model-menu, or model-turn checklist item was observed during automated preparation

## Prior installed preparation after Steps 34 and 35

### Source and prior process boundary

- Current HEAD: `e7216d92c58f50d0c9b967bf4123e877b922cf47`
- Step 34 commit: `fb4e0bfe` (`Converge chat sessions with live catalogs`)
- Step 35 commit: `e7216d92` (`Partition live hypotheses into disjoint fields`)
- Installed Workshop or Gateway processes observed before rebuild: 0
- Installed Workshop or Gateway processes stopped: 0

### Release Gateway

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
- Result: passed
- Summary: release profile finished in 26.57 seconds with 9 `gateway-stt` warnings
- Build started: `2026-09-07T09:39:44.2506384Z`
- Build finished: `2026-09-07T09:40:10.9436429Z`
- Artifact: `target/release/promptforge-gateway.exe`
- Last modified: `2026-09-07T09:40:10.4052397Z`
- Size: 14,477,824 bytes
- SHA-256: `D79453C2D91A6AF921C93C861624C8CCA4AC31497E1AF19AA38E458849E119DC`

### Target-suffixed sidecar

- Command: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
- Artifact: `crates/workshop/binaries/promptforge-gateway-x86_64-pc-windows-msvc.exe`
- Size: 14,477,824 bytes
- SHA-256: `D79453C2D91A6AF921C93C861624C8CCA4AC31497E1AF19AA38E458849E119DC`
- Verification: source and staged SHA-256 hashes matched at `2026-09-07T09:40:21.0845907Z`

### Packaging tool

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
- Result: passed
- Installed version: `tauri-cli 2.11.4`
- Detail: Cargo reported that the same version was already installed
- Verified: `2026-09-07T09:40:28.3091432Z`

### Fresh unsigned local NSIS package

- Exact successful PowerShell command: `cargo --% tauri build --bundles nsis --config {\"bundle\":{\"createUpdaterArtifacts\":false}}`
- Working directory: `crates/workshop`
- Result: passed
- Build started: `2026-09-07T09:40:48.847Z`
- Build finished: `2026-09-07T09:42:15.042Z`
- Workshop release profile finished in 59.28 seconds
- Installer: `target/release/bundle/nsis/PromptForge_0.2.0_x64-setup.exe`
- Installer created: `2026-09-07T09:41:51.6524557Z`
- Installer last modified: `2026-09-07T09:42:13.4020073Z`
- Installer size: 12,381,612 bytes
- Installer SHA-256: `CD114A03A98F5E9F3354DC889998742E01EB3C221DB0CA61F0AA835DBDED885D`
- Previous installer SHA-256: `CFBB5CBB539BE6B77FAB17BC9030E76CBA3D55B1DB21E84D5BD95CAF08E52606`
- Freshness proof: the installer creation and modification timestamps follow the successful build start, and its hash differs from the previous installer
- Override scope: `bundle.createUpdaterArtifacts=false` was supplied only through the Tauri command line
- Protected release configuration: `crates/workshop/tauri.conf.json` and `.github/workflows/release-workshop.yml` have no diff
- Signing: not tested

The adjacent `PromptForge_0.2.0_x64-setup.exe.sig` remains stale from `2026-09-06T02:42:54.1030502Z` and is excluded from this build's evidence.

### Silent installation and installed identities

- Install result: passed
- Installer exit code: 0
- Install started: `2026-09-07T09:42:28.9881969Z`
- Install finished: `2026-09-07T09:42:32.3954431Z`
- Workshop path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Workshop file version: `0.2.0`
- Workshop product version: `0.2.0`
- Workshop last modified: `2026-09-07T09:41:50Z`
- Workshop size: 24,290,816 bytes
- Workshop SHA-256: `0BCD250129F2D7B1BE5218326C7FEF8FC93238ACE69CC73AFFF6648FB0F6FE74`
- Build-tree Workshop size: 24,290,816 bytes
- Build-tree Workshop SHA-256: `22897B508E501402B4FF17A207917AD3BF30C573B374E2F4DA671BA8DA160EE8`
- Workshop comparison: both identities are recorded without claiming equality because the Tauri log records NSIS bundle-information patching during packaging
- Gateway sibling path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Gateway sibling last modified: `2026-09-07T09:40:10Z`
- Gateway sibling size: 14,477,824 bytes
- Gateway sibling SHA-256: `D79453C2D91A6AF921C93C861624C8CCA4AC31497E1AF19AA38E458849E119DC`
- Gateway verification: installed, staged, and release SHA-256 hashes match
- Identity verification observed: `2026-09-07T09:42:42.8152106Z`

### Installed application launch

- Launched: `2026-09-07T09:42:51.6396972Z`
- Readiness-window observation: `2026-09-07T09:43:11.7819091Z`
- Workshop process ID: 79208
- Gateway process ID: 60656
- Both process paths resolve under `C:\Users\Vinnie\AppData\Local\PromptForge`
- Both processes remained running at `2026-09-07T09:43:59.6143596Z`
- No physical microphone, model-menu, or model-turn checklist item was observed during automated preparation

## Prior installed preparation before Steps 34 and 35

### Source and release Gateway

- Current HEAD: `aeec7b48fad441f42e5b66ec09274f50455180eb`
- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
- Result: passed
- Summary: release profile finished in 27.14 seconds with 9 `gateway-stt` warnings
- Build started: `2026-09-07T07:22:02.4562698Z`
- Build finished: `2026-09-07T07:22:29.7203501Z`
- Artifact: `target/release/promptforge-gateway.exe`
- Last modified: `2026-09-07T07:22:29.1910374Z`
- Size: 14,459,904 bytes
- SHA-256: `8A1D73EDD4BE102482B5B7DAF253B09F6D90C51EA0EA13EE439ECAA4E9DF5DEB`

### Target-suffixed sidecar

- Command: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
- Artifact: `crates/workshop/binaries/promptforge-gateway-x86_64-pc-windows-msvc.exe`
- Size: 14,459,904 bytes
- SHA-256: `8A1D73EDD4BE102482B5B7DAF253B09F6D90C51EA0EA13EE439ECAA4E9DF5DEB`
- Verification: source and staged SHA-256 hashes matched at `2026-09-07T07:22:35.0127460Z`

### Packaging tool

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
- Result: passed
- Installed version: `tauri-cli 2.11.4`
- Detail: Cargo reported that the same version was already installed
- Verified: `2026-09-07T07:25:14.1899756Z`

### Fresh unsigned local NSIS package

- Authorized command: `cargo tauri build --bundles nsis --config '{"bundle":{"createUpdaterArtifacts":false}}'`
- Working directory: `crates/workshop`
- Result: passed
- Successful build started: `2026-09-07T07:22:58.5565020Z`
- Successful build finished: `2026-09-07T07:24:11.0138047Z`
- Installer: `target/release/bundle/nsis/PromptForge_0.2.0_x64-setup.exe`
- Installer created: `2026-09-07T07:23:52.9874435Z`
- Installer last modified: `2026-09-07T07:24:10.9102655Z`
- Installer size: 12,374,918 bytes
- Installer SHA-256: `CFBB5CBB539BE6B77FAB17BC9030E76CBA3D55B1DB21E84D5BD95CAF08E52606`
- Previous installer SHA-256: `750DBEF0F96FC9AE4364942856E558E2D951731CEE8BDC607397A36A457599AB`
- Freshness proof: the installer creation and modification timestamps follow the successful build start, and its hash differs from the previous installer
- Override scope: `bundle.createUpdaterArtifacts=false` was supplied only through the Tauri command line
- Protected release configuration: `crates/workshop/tauri.conf.json` and `.github/workflows/release-workshop.yml` have no diff
- Signing: not tested

The adjacent `PromptForge_0.2.0_x64-setup.exe.sig` remains stale from `2026-09-06T02:42:54.1030502Z` and is excluded from this build's evidence.

### Silent installation and installed identities

- Install result: passed
- Installer exit code: 0
- Install started: `2026-09-07T07:24:18.0065320Z`
- Install finished: `2026-09-07T07:24:21.3903424Z`
- Workshop path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Workshop version output: `promptforge-workshop 0.2.0`
- Workshop last modified: `2026-09-07T07:23:50Z`
- Workshop size: 24,245,248 bytes
- Workshop SHA-256: `42E7D7500425F91AE576F4E1CAE5E11DE0B606EF1836E8EC1A2EEABD059B7A73`
- Build-tree Workshop SHA-256: `A890D75817337D68A1E8660F8B11F11E7216A9D71C05625A712E9193E8E35CBD`
- Workshop comparison: both identities are recorded without claiming equality because the Tauri log records NSIS bundle-information patching during packaging
- Gateway sibling path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Gateway sibling last modified: `2026-09-07T07:22:28Z`
- Gateway sibling size: 14,459,904 bytes
- Gateway sibling SHA-256: `8A1D73EDD4BE102482B5B7DAF253B09F6D90C51EA0EA13EE439ECAA4E9DF5DEB`
- Gateway verification: installed, staged, and release SHA-256 hashes match
- Identity verification observed: `2026-09-07T07:24:36.4332156Z`

### Installed application launch

- Launched through Windows Explorer for operator handoff: `2026-09-07T07:26:12.8249425Z`
- Readiness-window observation: `2026-09-07T07:26:25.7625183Z`
- Separate post-handoff observation: `2026-09-07T07:26:37.3855897Z`
- Workshop process ID: 83436
- Gateway process ID: 98380
- Both process paths resolve under `C:\Users\Vinnie\AppData\Local\PromptForge`
- No physical microphone or model-turn checklist item was observed during automated preparation

## Completed automated prerequisites

### Gateway Realtime STT

- Command: `cargo test -p gateway --test it realtime_stt`
- Result: passed
- Summary: 9 passed, 0 failed, 0 ignored, 78 filtered out
- Finished: `2026-09-07T01:55:48.700Z`

### Workshop Realtime relay

- Command: `cargo test -p workshop-server --test it realtime_relay`
- Result: passed
- Summary: 7 passed, 0 failed, 0 ignored, 30 filtered out
- Finished: `2026-09-07T01:56:27.864Z`

### Workshop UI

- Working directory: `crates/workshop-server/ui`
- Command: `npm test`
- Result: passed
- Summary: 71 passed, 0 failed, 0 cancelled, 0 skipped
- Finished: `2026-09-07T01:56:05.209Z`

## Completed release preparation

### Release Gateway

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
- Result: passed
- Summary: release profile finished in 1 minute 52 seconds with 9 `gateway-stt` warnings
- Finished: `2026-09-07T01:58:30.071Z`
- Artifact: `target/release/promptforge-gateway.exe`
- Size: 14,457,344 bytes
- SHA-256: `7211CA8E7D71533274EADD6264A77781D1F893B746B76EB8A560280770A7120F`

### Target-suffixed sidecar

- Command: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
- Result: passed
- Artifact: `crates/workshop/binaries/promptforge-gateway-x86_64-pc-windows-msvc.exe`
- Size: 14,457,344 bytes
- SHA-256: `7211CA8E7D71533274EADD6264A77781D1F893B746B76EB8A560280770A7120F`
- Verification: source and staged SHA-256 hashes match

### Packaging tool

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
- Result: passed
- Installed version: `tauri-cli 2.11.4`
- Detail: Cargo reported that the same version was already installed
- Finished: `2026-09-07T01:58:42.463Z`

## Unsigned local NSIS build

- Authorized command: `cargo tauri build --bundles nsis --config '{"bundle":{"createUpdaterArtifacts":false}}'`
- Working directory: `crates/workshop`
- Result: passed
- Build started: `2026-09-07T02:17:35.0140268Z`
- Build finished: `2026-09-07T02:19:29.0773256Z`
- Installer: `target/release/bundle/nsis/PromptForge_0.2.0_x64-setup.exe`
- Installer created: `2026-09-07T02:19:10Z`
- Installer last modified: `2026-09-07T02:19:28Z`
- Installer size: 12,360,736 bytes
- Installer SHA-256: `D045D0C4F57702AB42C93BBE1CEEC810A900C1952DAD1DE456011854899E5520`
- Freshness proof: installer creation and modification timestamps are later than the successful attempt's start timestamp
- Override scope: `bundle.createUpdaterArtifacts=false` was supplied only on the Tauri command line
- Protected files: `crates/workshop/tauri.conf.json` and `.github/workflows/release-workshop.yml` have no diff
- Signing: not tested

The successful unsigned attempt did not create a current `.sig` file. The adjacent `PromptForge_0.2.0_x64-setup.exe.sig` is stale from `2026-09-06T02:42:54Z` and is excluded from this run's evidence.

## Silent installation

- Command: `Start-Process $setup.FullName -ArgumentList '/S' -Wait -PassThru`
- Result: passed
- Installer exit code: 0
- Started: `2026-09-07T02:19:41.9986519Z`
- Finished: `2026-09-07T02:19:45.3941424Z`

## Installed sibling verification

### Workshop

- Path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Size: 24,210,432 bytes
- SHA-256: `B0B8B3A7D732CBF3B1982AF1CB588DB20F3A0BECD5ED5A508F65EF6E26459B28`
- Version output: `promptforge-workshop 0.2.0`
- Sibling Gateway present: yes

The Workshop build-tree executable has the same size but SHA-256 `169639C71AD94018FCA0F37E7977B607508EBDE59FE89B5DB4EB34A85366361C`. This is not treated as an applicable byte-for-byte comparison because the Tauri log records patching the Workshop executable with NSIS bundle information during packaging. Both hashes are recorded rather than claiming equality.

### Gateway

- Path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Size: 14,457,344 bytes
- SHA-256: `7211CA8E7D71533274EADD6264A77781D1F893B746B76EB8A560280770A7120F`
- Staged sidecar SHA-256: `7211CA8E7D71533274EADD6264A77781D1F893B746B76EB8A560280770A7120F`
- Release Gateway SHA-256: `7211CA8E7D71533274EADD6264A77781D1F893B746B76EB8A560280770A7120F`
- Verification: installed, staged, and release Gateway hashes match

## Installed application launch

- Launched: `2026-09-07T02:20:39.1019801Z`
- Workshop process ID: 78444
- Gateway sibling process observed: yes
- Gateway process ID: 101204
- Both installed processes remained running at the automated handoff

## Second installed attempt after no-model-turn fix

### Source and process boundary

- Current HEAD: `49441166f580a3d6339532a3c1fd3c1205e484cd`
- Installed processes observed before rebuild: 0
- Installed processes stopped: 0
- Installed processes remaining before rebuild: 0
- Process boundary observed: `2026-09-07T04:26:54.2378279Z`

### Release Gateway and staged sidecar

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
- Result: passed
- Summary: release profile finished in 49.18 seconds with 9 `gateway-stt` warnings
- Finished: `2026-09-07T04:27:44.975Z`
- Release Gateway: `target/release/promptforge-gateway.exe`
- Release Gateway last modified: `2026-09-07T04:27:42.7135449Z`
- Release Gateway size: 14,457,344 bytes
- Release Gateway SHA-256: `7BE1C818B196A1C889ADE75D8AA09847404808C6530FFC7662DBBC26E10BAD18`
- Staged sidecar: `crates/workshop/binaries/promptforge-gateway-x86_64-pc-windows-msvc.exe`
- Staged sidecar size: 14,457,344 bytes
- Staged sidecar SHA-256: `7BE1C818B196A1C889ADE75D8AA09847404808C6530FFC7662DBBC26E10BAD18`
- Staging verified: `2026-09-07T04:27:55.0750924Z`
- Verification: current release and staged Gateway hashes match

### Fresh unsigned local NSIS package

- Authorized command: `cargo tauri build --bundles nsis --config '{"bundle":{"createUpdaterArtifacts":false}}'`
- Working directory: `crates/workshop`
- Result: passed
- Previous installer last modified: `2026-09-07T02:19:28.9716724Z`
- Previous installer SHA-256: `D045D0C4F57702AB42C93BBE1CEEC810A900C1952DAD1DE456011854899E5520`
- Build started: `2026-09-07T04:28:04.7578431Z`
- Build finished: `2026-09-07T04:29:35.1883254Z`
- Current installer: `target/release/bundle/nsis/PromptForge_0.2.0_x64-setup.exe`
- Current installer created: `2026-09-07T04:29:17Z`
- Current installer last modified: `2026-09-07T04:29:35Z`
- Current installer size: 12,359,149 bytes
- Current installer SHA-256: `750DBEF0F96FC9AE4364942856E558E2D951731CEE8BDC607397A36A457599AB`
- Freshness proof: the current installer creation and modification timestamps follow this attempt's start, and its hash differs from the previous installer
- Override scope: `bundle.createUpdaterArtifacts=false` was supplied only on the Tauri command line
- Protected release configuration: unchanged
- Signing: not tested

### Second silent installation

- Result: passed
- Installer exit code: 0
- Started: `2026-09-07T04:29:47.7712059Z`
- Finished: `2026-09-07T04:29:51.1584956Z`

### Second installed identities

- Workshop path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Workshop version output: `promptforge-workshop 0.2.0`
- Workshop file version: `0.2.0`
- Workshop last modified: `2026-09-07T04:29:14Z`
- Workshop size: 24,211,456 bytes
- Workshop SHA-256: `AD2A99A912F016C7B11B37DEAA29DF6A04AB2292BD8C56E1C3D6065585D29B35`
- Build-tree Workshop SHA-256: `A0638A0B723997026CFE92DC056597E13D83DBB032B18B8D7453DC2AE513246F`
- Workshop comparison: both identities are recorded without claiming equality because the Tauri log records NSIS bundle-information patching during packaging
- Gateway sibling path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Gateway sibling last modified: `2026-09-07T04:27:42Z`
- Gateway sibling size: 14,457,344 bytes
- Gateway sibling SHA-256: `7BE1C818B196A1C889ADE75D8AA09847404808C6530FFC7662DBBC26E10BAD18`
- Gateway verification: installed, staged, and release SHA-256 hashes match
- Identity verification observed: `2026-09-07T04:30:01.8473944Z`

### Second installed launch

- Launched: `2026-09-07T04:30:08.4151156Z`
- Workshop process ID: 48564
- Gateway process ID: 59724
- Both process paths resolve under `C:\Users\Vinnie\AppData\Local\PromptForge`
- Both processes remained running at `2026-09-07T04:30:16.5398864Z`

## Operator observations - later build-tree launch

- Observed: approximately `2026-09-07T06:33Z` through `2026-09-07T06:36Z`
- Acceptance applicability: none; process inspection showed both Workshop and Gateway running from `C:\Users\Vinnie\cursor\promptforge\target\release`, not the installed `AppData\Local\PromptForge` paths
- Gateway readiness: serving at `06:33:11Z`, speech ready with GPU and profile switched by `06:33:13Z`, `claude-opus-4-6` advertised, chat endpoint ready
- Workshop model catalog: failed; the picker exposed no model even though Gateway advertised `claude-opus-4-6`
- Realtime connection: failed latency; microphone readiness took approximately 20 to 30 seconds
- Live hypotheses: failed; no text evolved while recording
- Completion: functional; correct text appeared only after stop
- Status lifecycle: failed; the progress bar remained visible after profile completion and the normal LEDs did not return
- Diagnosis: Workshop refreshed model state before Gateway profile publication and did not retry while health stayed reachable; precommit hypothesis IDs were not bound to the active take; the imported Gateway progress operation remained attached to the never-ending SSE stream after its root finished

## Prior failed observations - first installed attempt

### Dictation

- Observed: approximately `2026-09-07T03:55Z` through `2026-09-07T03:57Z`
- Result: partial success, latency failure
- Evidence: the installed Workshop first displayed `Dictation is connecting. Try again in a moment.`, then eventually inserted `Tell me a story, is it gonna work? I don't think it's gonna work.`
- Connection delay: 5 to 15 seconds
- Stop-to-final delay: 5 to 15 seconds
- Verdict: the installed speech path works functionally, but both observed delays exceed the two-second acceptance budget

### Model turn after dictation

- Observed: approximately `2026-09-07T03:57Z`
- Result: failed
- Evidence: the model picker still displayed `Select model`; submission persisted the user message and a tool result containing the dictated text, but no assistant message followed
- Session log: `dd27eef4544a74d2b12e9f1a25251000`
- Gateway state during diagnosis: running, profile `default`, `claude-opus-4-6` advertised, chat endpoint ready, speech ready with GPU, no active or pending command
- Network state during diagnosis: Workshop retained local Gateway connections, while Gateway held no outbound provider connection
- Verdict: no Anthropic request was reached; the local no-model binding error returned through Lua `pcall` without an operator-visible response

## Operator observation checklist retained for audit context

This checklist records the detail originally requested for the post-Step 37 build. Unchecked items were not individually recorded and are not retroactively claimed as measured. The post-Step 37 verdict below remains historical evidence and does not complete the later Step 42 repeat.

- [ ] Confirm both chat model menus, the inline dropdown and the top-level `Model` menu, show only chat-capable models and do not list speech-only models.
- [ ] Select `claude-opus-4-6`, submit typed input, and confirm the selected Claude model completes the turn with an assistant response.
- [ ] Confirm live speech revisions replace rather than duplicate provisional text, with exact spacing preserved.
- [ ] Confirm completion commits the final transcript exactly once.
- [ ] Start a second take and confirm it is independent of the first.
- [ ] Clear the transcript and confirm the visible and retained take state clears.
- [ ] Cancel an active take and confirm no later hypothesis or completion is applied.
- [ ] Deny microphone permission or select an unavailable device, confirm a recoverable error, restore access, and confirm a new take works.
- [ ] Measure connection delay from microphone activation to ready capture.
- [ ] Measure stop-to-final delay from stop action to committed final transcript.
- [ ] Record the installed Workshop path, sibling Gateway path, installer path, sizes, SHA-256 hashes, and UTC timestamps.

Automated preparation did not perform the checklist. The operator later observed the post-Step 37 installed build and accepted it as recorded below, without supplying measurements or item-by-item results beyond those stated.

## Operator acceptance - post-Step 37 installed build

- Observed: approximately `2026-09-07T12:15Z`
- Build under test: installed unsigned package built from `2d1ecca8`
- Short-utterance Stop regression: passed; repeated utterances with the last word spoken immediately before Stop retained the correct final word
- Live transcription: passed; operator reported the repaired behavior works correctly
- Overall operator verdict: `Works correctly. Accepted.`
- Signing: not tested; release signing remains a release-CI gate

## Operator observations - post-Step 36 installed attempt

- Observed: approximately `2026-09-07T10:59Z`
- Live hypotheses: substantially improved; the prior repeated-phrase accumulation was not observed
- Stop finalization: failed intermittently; a correct word appeared in the latest live hypothesis, then pressing Stop removed that word from the authoritative completion
- Verdict: cadence and whole-window rebasing improved the live path, but Step 37 remains failed because completion can discard recognized audio-backed tail text

## Operator observations - post-Steps 34 and 35 installed attempt

- Observed: approximately `2026-09-07T09:47Z`
- Chat model menus: passed; speech-only models no longer appeared
- Typed model turn: passed; selected chat model responded
- Live hypotheses: failed; provisional text still accumulated repeated phrases while recording instead of presenting one evolving replacement
- Completion: prior behavior indicates Stop replaces provisional text with the clean authoritative final, but the full completion checklist was not repeated in this observation
- Verdict: Step 34 repairs passed installed observation; Step 35 did not repair the real native interim sequence, so Step 36 remains failed

## Prior failed observations - pre-Steps 34 and 35 installed attempt

- Observed: approximately `2026-09-07T08:45Z`
- Model catalog: `claude-opus-4-6` was visible and selected
- Chat model menus: failed filtering; both the inline dropdown and top-level `Model` menu listed `whisper-base-en`, `whisper-small-en`, and `realtime-transcribe`, which are speech models and must not be selectable for chat
- Typed model turn: failed; submitting `test 1 2 3` persisted the user input and tool result, then displayed `Error: Model turn failed in agent 'chat'`
- Model-turn diagnosis: Workshop launched the built-in chat session before Gateway published its profile models, freezing an empty session model catalog; later catalog convergence updated the picker but not that running session, so binding failed locally before any Gateway completion request
- Live hypotheses: failed replacement behavior; revisions appeared while recording but accumulated repeatedly in the editor
- Completion: functional replacement; pressing Stop removed the duplicated provisional text and left the correct final transcript
- Verdict: Step 34 remains failed; model-session catalog convergence and live ProseMirror range replacement require repair before acceptance can be repeated

## Step 42 full release verification at HEAD d79823ed

### Run boundary

- Current commit: `d79823ed723b155a77d704e8861c1f1e7e00e6c1` (`Bookend Gateway serving file logs`)
- Shell: Windows PowerShell `5.1.26100.9278`
- Initial gate toolchain: Cargo `1.89.0`; release commands explicitly selected stable
- Other tools: Node `v24.19.0`, npm `11.17.0`, mdBook `0.4.44`, Tauri CLI `2.11.4`, cargo-deny `0.20.2`, cargo-modules `0.25.0`, cargo-public-api `0.52.0`
- Initial worktree: clean
- Commit created: no

### Rust, policy, build, and native gates

- Command: `cargo fmt --all --check`
  - Result: passed, exit code 0, 3.815 seconds, no output
- Command: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - Result: passed, exit code 0, 34.739 seconds
  - Summary: finished the development profile in 32.48 seconds with no warning or error
- Command: `cargo test --workspace`
  - Result: passed, exit code 0, 265.410 seconds
  - Summary: every workspace unit, integration, binary, and documentation target completed without a failed test
- Command: `cargo test --workspace --all-features --doc`
  - Result: passed, exit code 0, 174.292 seconds
  - Summary: every all-feature documentation target completed without a failed test
- Command: `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features`
  - Result: passed, exit code 0, 19.744 seconds
  - Summary: finished in 17.63 seconds and generated 33 documented workspace entries with warnings denied
- Command: `cargo deny check`
  - Result: passed, exit code 0, 11.997 seconds
  - Exact terminal summary: `advisories ok, bans ok, licenses ok, sources ok`
  - Permitted warnings included duplicate and wildcard dependency reports, one license-not-encountered report, and yanked `chacha20 0.10.1`
- Command: `cargo build -p gateway`
  - Result: passed, exit code 0, 7.171 seconds
  - Summary: finished in 5.22 seconds with 11 default-feature `gateway-stt` unused or dead-code warnings
- Command: `cargo build -p workshop`
  - Result: passed, exit code 0, 39.271 seconds
  - Summary: finished in 37.20 seconds
- Command: `cargo check -p gateway --no-default-features`
  - Result: passed, exit code 0, 3.776 seconds
  - Summary: featureless Gateway finished in 1.71 seconds
- External native prerequisite check:
  - `C:\Users\Vinnie\cursor\promptforge\local\stt-fixtures\whisper.dll`: present, 1,368,064 bytes
  - `C:\Users\Vinnie\cursor\promptforge\crates\gateway-stt-backend-whisper\tests\fixtures\ggml-tiny.en.bin`: present, 77,704,715 bytes
  - `C:\Users\Vinnie\cursor\promptforge\crates\gateway-stt-backend-whisper\tests\fixtures\jfk.wav`: present, 352,078 bytes
- Command: `$fixture=(Resolve-Path 'local\stt-fixtures').Path; $env:PATH="$fixture;$env:PATH"; $env:PROMPTFORGE_WHISPER_LIBRARY=(Resolve-Path 'local\stt-fixtures\whisper.dll').Path; cargo test -p gateway-stt-backend-whisper --test native_whisper -- --ignored`
  - Result: passed, exit code 0, 9.969 seconds
  - Native equivalence: 5 passed, 0 failed, 0 ignored, including the fixed JFK transcript, glossary and transcript conditioning, stateless-job independence, absent-final classification, and model-progress terminals
- Command: `cargo +nightly-2026-09-05 miri test -p gateway-stt-engine --features test-fixtures miri_`
  - Result: passed, exit code 0, 10.565 seconds
  - Summary: 2 passed, 0 failed, 20 filtered out; contract and cleanup targets had 0 selected tests
- Command: `cargo +nightly-2026-09-05 miri test -p gateway-stt --features test-fixtures miri_`
  - Result: passed, exit code 0, 41.628 seconds
  - Summary: 11 passed, 0 failed, 54 filtered out

### Architecture ratchets

- Command: `node tools/check-stt-architecture.mjs`
  - Result: passed, exit code 0, 40.759 seconds
  - `gateway-stt`: acyclic, 6 public roots, 43 source modules, largest module 481 lines at `realtime/session.rs`
  - `gateway-stt-engine`: acyclic, 7 public roots, 10 source modules, largest module 460 lines at `worker.rs`
  - `gateway-stt-backend-whisper`: acyclic, 2 public roots, 4 source modules, largest module 293 lines at `model.rs`
  - `gateway-whisper-ffi`: acyclic, 6 public roots, 7 source modules, largest module 226 lines at `context.rs`
- Command: `cargo test -p gateway-stt --test it architecture`
  - Result: passed, exit code 0, 2.623 seconds
  - Summary: 16 passed, 0 failed, 40 filtered out; exact final dependencies, exact ceilings, final migration state, unsafe isolation, generation ownership, and legacy seam removal all passed

### UI gates

- Workshop UI command: `npm run typecheck`
  - Result: passed, exit code 0, 3.531 seconds; `check-layers: ok`
- Workshop UI command: `npm run build`
  - Result: passed, exit code 0, 3.329 seconds; emitted `dist/app.js` at 2.5 MiB and `dist/app.css` at 166.0 KiB
- Workshop UI command: `npm test`
  - Result: passed, exit code 0, 9.077 seconds; 69 passed, 0 failed, 0 cancelled, 0 skipped
- Gateway config UI command: `npm run typecheck`
  - Result: passed, exit code 0, 3.316 seconds
- Gateway config UI command: `npm run build`
  - Result: passed, exit code 0, 2.713 seconds; emitted `dist/app.js` at 291.4 KiB and `dist/app.css` at 37.7 KiB
- Gateway config UI command: `npm test`
  - Result: passed, exit code 0, 18.080 seconds; `check-layers: ok`; 128 passed, 0 failed, 0 cancelled, 0 skipped

### Generated documentation

- Command: `cargo run -p build-user-guide`
  - Result: passed, exit code 0, 2.195 seconds
- Command: `mdbook build guide`
  - Result: passed, exit code 0, 2.272 seconds; HTML backend completed
- Command: `git diff --exit-code -- guide/src/SUMMARY.md guide/src/gateway/index.md guide/src/workshop/index.md guide/src/language/index.md guide/src/agent/index.md guide/promptforge-gateway-guide.md guide/promptforge-workshop-guide.md guide/promptforge-language-guide.md guide/promptforge-agent-guide.md`
  - Result: passed, exit code 0, with only Git line-ending notices for the Workshop and Agent single-file guides
- Generated-doc cleanliness: all nine SHA-256 values were identical before and after regeneration:
  - `guide/src/SUMMARY.md`: `4031AACD9459ED213C3E5D41466993691FD8B2DA07DEC9D090D90E8493F99FFC`
  - `guide/src/gateway/index.md`: `09E9807249611001CA6CAF2A1A210BF64E2B843C4C6B6A8C7284068F6E44B2D2`
  - `guide/src/workshop/index.md`: `4BC7756A0D6D66807061BD747C72618096B24ACC5031F536F12B4019C53F4226`
  - `guide/src/language/index.md`: `41E9E4458BC1ED9F969F0DB7E13C24A3D94A4E88253D88239E6AA3F40F631AFD`
  - `guide/src/agent/index.md`: `1C66A4A2EF1AF16AE38668F2D0910124F20520E714610EA8B150C282ECAC623E`
  - `guide/promptforge-gateway-guide.md`: `5BF95CD9776A87982E13D9C6E7DA09F7BED2375292E75919F963BF59F497BCDE`
  - `guide/promptforge-workshop-guide.md`: `F45DB5FBAE9B56CB4415218CBA2B8D12EEAB39B45DEF10922349C369E2921DF0`
  - `guide/promptforge-language-guide.md`: `3CDF6E562EF45AC8873703C81701834650CAC7E8A8459839E96466D03AF16DFA`
  - `guide/promptforge-agent-guide.md`: `3B70D4DE22FF4077BC31D9E484BC8672DDF256413795B6BD8708774DB29464F9`

### Fresh unsigned NSIS package

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
  - Result: passed, exit code 0, 42.019 seconds; release profile finished in 39.64 seconds with 11 `gateway-stt` warnings
- Release Gateway:
  - Path: `C:\Users\Vinnie\cursor\promptforge\target\release\promptforge-gateway.exe`
  - Last modified: `2026-09-07T13:53:06.4583106Z`
  - Size: 13,405,696 bytes
  - SHA-256: `C60DFDEBC6E45EEE82AF6952FF81CF1B0A1F0B92ADC8EA1BFE68F82105E8C06B`
- Command: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
  - Result: passed, exit code 0
  - Staged sidecar size and SHA-256 exactly matched the release Gateway
- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
  - Result: passed, exit code 0, 2.351 seconds; Tauri CLI `2.11.4` was already installed
- Exact PowerShell 5.1 package command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo --% tauri build --bundles nsis --config {\"bundle\":{\"createUpdaterArtifacts\":false}}`
  - Result: passed, exit code 0, 107.781 seconds; release profile finished in 1 minute 11 seconds and produced one NSIS bundle
  - Override scope: `bundle.createUpdaterArtifacts=false` was supplied only through the command line
  - Protected release configuration: `crates/workshop/tauri.conf.json` and `.github/workflows/release-workshop.yml` remained clean
- Fresh installer:
  - Path: `C:\Users\Vinnie\cursor\promptforge\target\release\bundle\nsis\PromptForge_0.2.0_x64-setup.exe`
  - Created: `2026-09-07T13:55:16.3424357Z`
  - Last modified: `2026-09-07T13:55:37.7434332Z`
  - Size: 12,034,196 bytes
  - SHA-256: `194D2D6D86C6E552E12E1E7D96E5469914FE11E588B123E6313833E0C49A9F78`
  - Previous installer SHA-256: `CE476DE44A6F7E0897765ED45AA6E988702826FC9F4B7083A155DBE90E90F028`
  - Freshness: creation and modification followed the package start, and the hash changed
  - Signing: not tested; adjacent `.sig` is stale from `2026-09-06T02:42:54.1030502Z` and is excluded

### Installation

- Command: `$setup=Get-ChildItem -Recurse 'target\release\bundle\nsis' -Filter '*-setup.exe' | Select-Object -First 1; if (-not $setup) { throw 'no NSIS installer' }; $process=Start-Process $setup.FullName -ArgumentList '/S' -Wait -PassThru; if($process.ExitCode -ne 0){throw "installer exited $($process.ExitCode)"}`
  - Result: passed; installer exit code 0
- Installed Workshop:
  - Path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
  - Last modified: `2026-09-07T13:55:14Z`
  - Size: 24,186,880 bytes
  - SHA-256: `94BBB68A5E0C71CE6FFD0FA5014C13E0DF9B17ECE15C80A8B1544991ED7EBCCB`
  - Product version: `0.2.0`
- Installed Gateway:
  - Path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
  - Last modified: `2026-09-07T13:53:06Z`
  - Size: 13,405,696 bytes
  - SHA-256: `C60DFDEBC6E45EEE82AF6952FF81CF1B0A1F0B92ADC8EA1BFE68F82105E8C06B`
  - Native package equivalence: installed, staged, and release Gateway hashes match exactly

### Installed launch and operator boundary

- Installed Workshop launch: passed
- Handoff observed: `2026-09-07T14:00:32.5494104Z`
- Workshop process: PID 92768 at `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Gateway process: PID 65268 at `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Readiness observation: both installed processes remained running 20 seconds after launch
- Automated Step 42 gates: passed
- Physical observation completed: microphone recording and chat turns on the reopened installed Workshop
- Remaining physical scenarios:
  - Confirm a second take is independent
  - Confirm clear removes visible and retained take state
  - Confirm cancellation prevents later hypothesis or completion application
  - Confirm permission denial or unavailable-device failure is recoverable, then restore access and complete a new take
- Signing: not tested
- Handoff note: after the recorded readiness check the Workshop window closed while Gateway PID 65268 remained running. The installed Workshop was reopened as PID 73520 after the `2026-09-07T14:00:32.5494104Z` handoff and before the approximately `2026-09-07T14:03Z` observation. Its exact process start timestamp was not retained, and PID 73520 is no longer running.

### Final installed operator acceptance

- Observed: approximately `2026-09-07T14:03Z`
- Package under test: the fresh Step 42 unsigned installer recorded above
- Installed Workshop process: PID 73520
- Observed scope: microphone recording and chat turns
- Operator verdict for that scope: `works beautifully`
- Not independently observed: second-take independence, Clear, cancellation, and permission-denial or unavailable-device recovery
- Physical acceptance: incomplete pending those four scenarios
- Signing: not tested; release signing remains a release-CI gate
