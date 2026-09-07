# PromptForge architecture queue

- [2026-07-29-1-gateway-v0] caller-model alias: Restore the caller's model name after upstream routing; this may remain a wire-level detail.
- [2026-07-29-4-webfetch-crate-extraction] fetch provenance contract: Return final URL, truncation, and extraction mode with fetched text so consumers know what they received.
- [2026-07-29-5-multi-turn-research-prompt] prompt-owned tool budget: Let a prompt declare its tool-iteration budget while retaining a finite default.
- [2026-07-29-5-multi-turn-research-prompt] soft output target: A prose token target is guidance, not an enforced output limit.
- [2026-07-29-6-per-section-tool-scoping] add-only scope API: The current Lua scoping surface accumulates and deduplicates names but has no remove or clear operation.
- [2026-07-29-7-guard-wrap-untrusted-output] nonce delimiter mechanics: Random XML-style tags and escaping are model-specific mitigation details, not an isolation guarantee.
- [2026-07-31-1-orchestrator-only] three-axis budgets: Track nesting, section transitions, and tool calls independently rather than using one global limit.
- [2026-08-02-1-tool-picker] picker threshold calibration: Similarity floors and duplicate thresholds are workload evidence, not general architecture.
- [2026-08-03-1-mcp-server-correction] reuse before machinery: The existing principle already requires Lua, store, or catalog reuse before new configuration or APIs.
- [2026-08-03-1-mcp-server-correction] as-built document ownership: Co-locate each existing crate's as-built design document and keep unbuilt design as separate residue.
- [2026-08-04-1-recover-core-design-rationale] irrecoverable contingency: Exact thresholds, external observations, and deleted alternatives cannot be reconstructed reliably from current code.
- [2026-08-05-1-section-lua-lifecycle] exact declaration replay: Freeze capability bindings once and replay declarations without resolving again.
- [2026-08-05-1-section-lua-lifecycle] observer side channel: Observation must remain non-blocking, payload-free, and irrelevant to execution decisions.
- [2026-08-06-1-prompt-fixtures-logging] bounded author logging: Author logs are the sole payload-bearing observation exception and stay bounded and single-line.
- [2026-08-07-1-dev-loop] explicit development workflow: Real inference and artifact provisioning belong to explicit developer commands.
- [2026-08-07-1-dev-loop] output channel discipline: Developer results use stdout; traces and diagnostics use stderr.
- [2026-08-07-2-store-type-rename] type names are not architecture: Store implementation renames should not alter the run-scoped filesystem contract.
- [2026-08-07-3-models-debug-cluster] frozen model binding: Bind prompt-local model aliases from gateway catalog metadata once per run.
- [2026-08-07-3-models-debug-cluster] constraint and invocation split: Capability constraints filter models; sampling settings travel with each invocation.
- [2026-08-07-4-completion-normalize-layer] dialect concentration: Provider response quirks belong at one protocol normalization boundary.
- [2026-08-07-5-store-and-fanout] trust-selective store reads: Raw reads serve trusted code; model-facing reinjection uses an untrusted envelope.
- [2026-08-07-5-store-and-fanout] explicit cross-section handoff: State crosses through the store or named reply and item payloads, never shared Lua memory.
- [2026-08-08-2-webfetch-soft-errors] error body exclusion: HTTP error bodies never enter model-facing recovery results.
- [2026-08-08-3-gateway-local-inference] bounded resource scheduling: Concurrency is limited at device or lane boundaries with visible backpressure.
- [2026-08-08-3-gateway-local-inference] inference engine attachment: Sidecar versus in-process inference is an implementation choice behind the gateway boundary.
- [2026-08-08-4-add-sys-model] host transport role: Hosts supply gateway location and credentials while prompts own capability and inference choices.
- [2026-08-08-5-extract-dev-crate] narrow developer interface: Prompt files own model knobs; the development command owns only file, input, and watch concerns.
- [2026-08-08-6-local-toml-bigger-model] profile composition: Resolve includes relative to the declaring profile; child objects replace by identity and otherwise append.
- [2026-08-08-7-gateway-download-progress] progress observation seam: Artifact transfer reports through a replaceable observer independent of the download loop.
- [2026-08-08-8-tool-dialect-plugins] dialect ambiguity is admission failure: A local tie or no match must fail visibly instead of silently choosing a protocol.
- [2026-08-08-9-write-through-store-traces] mirror without core coupling: Keep live developer dumps behind the existing Store and observation contracts.
- [2026-08-08-10-fanout-gateway-concurrency] gateway-owned backpressure: Let the gateway queue throttle model work unless non-model arm work proves it needs a host cap.
- [2026-08-08-11-briefer-evidence-thickening] shallow parallel research: Prefer focused bounded arms over deeper exploratory tool loops.
- [2026-08-08-11-briefer-evidence-thickening] concurrent source aggregation: Defer merged append logs until their write ordering is defined.
- [2026-08-08-12-supervise-local-llama-server] no background watchdog: Prefer bounded request-time supervision while continuous health selection is unnecessary.
- [2026-08-09-1-gateway-web-search-upgrade] deterministic result shaping: Validate, sanitize, filter, and diversify results before exposing them to a model.
- [2026-08-09-1-gateway-web-search-upgrade] cache remains absent: Add search-result caching only after repeat workloads justify a coherency policy.
- [2026-08-09-2-promptforge-user-guide] reference parity: Record availability, signatures, return values, and examples for every public prompt-language surface.
- [2026-08-09-3-readme-and-ci-overhaul] documentation layering: Keep the landing page concise and move verified internals to development documentation.
- [2026-08-09-3-readme-and-ci-overhaul] visuals carry no contract: Presentation assets may frame documentation but cannot be the only architecture record.
- [2026-08-09-4-store-real-plus-virtual] capability object snapshots: Tool and model handles are first-class values whose execution snapshot changes only with scope generation.
- [2026-08-09-5-h1-once-no-replay] frozen per-section installation: Install resolved capabilities from Rust while serialized variables and the run store carry state.
- [2026-08-10-1-crate-review] continuous fix stage: Treat bounded implementation slices as one effort that continues until every finding is dispositioned and the build is clean.
- [2026-08-10-1-crate-review] green-before-advance: Never checkpoint or leave a component while its verification gate is red or blocked.
- [2026-08-11-1-dokuman-all-crates] test-only documentation: Omit a separate user guide when a crate exposes test infrastructure but no standalone user-facing capability.
- [2026-08-11-2-make-user-guide-crate] document structure preservation: Keep one product H1, transform component headings outside code fences, and fail visibly on missing source guides.
- [2026-08-12-1-mdbook-guide] documentation layers: Keep package READMEs as concise signposts while the built guide owns task-oriented documentation and API docs own signatures.
- [2026-08-12-1-mdbook-guide] release sequencing: Complete guide, package metadata, and dry-run validation before changing the release version or publishing artifacts.
- [2026-08-14-1-file-backed-store] backend-transparent execution: Keep the executor unaware of whether a run-scoped store is memory-backed or file-backed.
- [2026-08-14-1-file-backed-store] prompt-owned resume: Express resume and skip behavior through ordinary store operations in prompt code, not special engine logic.
- [2026-08-15-1-store-input-files] declared file contract: Prompts declare logical input and output paths while hosts choose how external content is supplied or returned.
- [2026-08-15-1-store-input-files] best-effort declared output: Report a missing declared output distinctly without retroactively failing an otherwise completed run.
- [2026-08-15-2-quickref-file] compressed operational reference: Keep an LLM-loadable quick reference factual and point each schema or language topic to its authoritative source.
- [2026-08-15-3-gateway-dotenv-support] config-matched secret sidecars: Associate an optional ignored secret file with each config layer by name rather than adding secret syntax to config.
- [2026-08-15-3-gateway-dotenv-support] gateway-local secret loading: Keep vendor secret loading inside the gateway; callers retain only gateway connection credentials.
- [2026-08-15-4-h1-only-prompt-support] remove inert fallback configuration: Do not retain declarative fields that execution no longer honors; fall-through uses actual reply state.
- [2026-08-16-1-fanout-scope-refactor] fanout input normalization: Accept either a section reference or an explicit string array at the Lua boundary, then execute fanout on normalized items.
- [2026-08-16-1-fanout-scope-refactor] parsed-list reuse: Expose section items from the parsed prompt instead of reparsing Markdown in author code.
- [2026-08-18-1-tools-local] section-lifetime host APIs: Install store, logging, and control callbacks once per section VM so all chunks and handlers share one coherent environment.
- [2026-08-18-2-empty-turn-and-mcp-env] soft sidecar parse failure: A missing or malformed optional env sidecar does not block config loading when required values resolve elsewhere.
- [2026-08-18-3-untrusted-global-vm-reorder] inclusive store ranges: Keep public line bounds 1-based, inclusive, and clamping so citation call sites copy source coordinates directly.
- [2026-08-18-4-fanout-collection-items] level-local control graph: Every section address sees only siblings and direct children; explicit transfer enters a level-local walk and execute contains its chain.
- [2026-08-19-1-collapse-fanout-arm-engine] adapter delta boundary: Arm-only identity, cancellation, observation, exhaustion, and outcome mapping stay in a thin driver instead of policy switches in the engine.
- [2026-08-19-2-promptforge-refactor] pure direct infer: Every infer call is one fresh tool-free turn and never mutates reply or sys; tool use requires an executed section.
- [2026-08-20-1-fix-core-review-findings] alias uniqueness: Reject local tool aliases that collide with declared aliases or earlier local registrations at the authoring line.
- [2026-08-20-1-fix-core-review-findings] malformed syntax containment: Grammar mismatches and bad offsets return categorized errors instead of reaching panic paths.
- [2026-08-22-1-runcontext-seed] post-H1 capability freeze: H1 owns the only binding writers; later execution sees read-only views and self-contained bindings.
- [2026-08-23-1-dominion-refactor] canonical gateway protocol: Clients speak one OpenAI-shaped protocol while backend dialect and thinking translation remain gateway-internal and incremental.
- [2026-08-23-2-gateway-phases-4-5] transport causality: Distinguish never-connected failures from mid-flight failures that may already have reached or billed the provider.
- [2026-08-24-1-stage-1-the-window] unsettled presentation transport: SSE and vanilla DOM were stage-one choices later challenged by WebSockets and TypeScript, so retain no architecture claim yet.
- [2026-08-24-2-autogen-config-first-run] observable graceful degradation: One threaded observer reports subsystem state, and gateway loss disables dependent features without stopping local Workshop functions.
- [2026-08-24-3-voice-ux-fixes] semantic activity signals: Status activity describes model state, not transport identity - thinking, generating, recording, and general are distinct user meanings.
- [2026-08-24-3-voice-ux-fixes] editable voice result: Final transcription replaces interim text in the normal auto-growing composer so the user can inspect and edit it before sending.
- [2026-08-24-4-progressive-transcription] capped vocabulary bias: Domain vocabulary is a soft glossary prompt shared by interim and final decoding and truncated to the model prompt budget.
- [2026-08-24-5-stop-recording-on-send] close without stop: A mid-recording submit closes the voice socket without requesting a final decode because the visible composer text is authoritative.
- [2026-08-24-6-ci-and-hygiene-fixes] platform-aware CI: Portable crates and native Workshop crates require separate platform and feature verification lanes.
- [2026-08-25-1-rename-workbench-to-workshop] lexical rename guard: Product abbreviation changes must not rewrite WebSocket route, feature, or local-variable tokens.
- [2026-08-26-1-remove-voice-status-line] zone affinity: Panel types have default zones, user placement persists, and the status bar stays outside the dock.
- [2026-08-26-1-remove-voice-status-line] auditable agent feed: Reasoning and tool activity remain inspectable without allowing transient rows to shift the composer.
- [2026-08-26-2-model-turn-actions] layout identity only: Restored layout preserves panel identity and placement, not in-memory chat history.
- [2026-08-26-3-workshop-regression-fixes] drag coexistence: Native file-drop interception must delegate page drag events so internal Dockview dragging survives.
- [2026-08-26-4-workshop-idiom-refactor] bounded boot queue: Pre-ready state pushes replay in arrival order from a capped queue that is cleared on disconnect.
- [2026-08-26-4-workshop-idiom-refactor] typed protocol twins: Rust and TypeScript protocol definitions are explicit peers; generation waits for demonstrated drift.
- [2026-08-27-1-refresh-tree-on-drop] cache scope: Root invalidation preserves independent expansion and subdirectory caches.
- [2026-08-27-2-workshop-server-refactor] single socket owner: One select loop owns each WebSocket and cleanup follows resource guards rather than exit-path calls.
- [2026-08-27-2-workshop-server-refactor] bounded shutdown: Serve handles expose readiness, shutdown, stopped, and join phases with a force watchdog outside library code.
- [2026-08-27-3-server-driven-menu-state] multiplexed chats: Tagged chats interleave with per-chat ordering and scoped cancellation while inbound events remain readable.
- [2026-08-27-3-server-driven-menu-state] profile-local memory: The server persists each profile last selected model outside hand-edited config.
- [2026-08-28-1-fix-open-vibe-findings] single boot parse: Gateway startup resolves the include chain once for both server and Workshop boot sections.
- [2026-08-28-2-coroutine-protocol-executor] cooperative scheduler boundary: One prompt run interleaves chains only at host I/O yields while keeping scheduler state exclusively owned.
- [2026-08-28-3-digest-marker-child-priority] child process priority: Below-normal llama-server priority improves CPU and I/O responsiveness but cannot cure display-driver contention.
- [2026-08-28-4-cuda-llama-provisioning] embedded native bundle: A canonical manifest binds source, toolchain, target, options, dependencies, and file digests before atomic staging.
- [2026-08-28-4-cuda-llama-provisioning] explicit companion artifacts: Draft and projector models carry independent sources and pins through the same provision and respawn lifecycle.
- [2026-08-29-1-crate-extraction-execution] versioned UI artifact: Rust builds consume a hash-verified UI bundle while release builds preserve a one-command path.
- [2026-08-29-2-rename-ws-crates-to-workshop] atomic product rename: Package, binary, directory, UI, and CI names move together while historical decision records keep their dated identity.
- [2026-08-29-3-chat-ws-decomposition] responsibility ratchet: Module ceilings prevent size regrowth but ownership drift still requires a rename or split.
- [2026-08-29-4-progress-architecture-rollout] terminal progress authority: Intermediate samples may coalesce, but sticky terminal outcomes and reconnect snapshots define completion.
- [2026-08-29-5-gateway-config-spa] loopback config wall: Sensitive config and secret APIs remain loopback-only even when bearer-authenticated.
- [2026-08-29-5-gateway-config-spa] app-owned configuration: Managed TOML and env files are canonical machine output rather than comment-preserving hand-edited documents.
- [2026-08-30-1-chat-templates-injection-defense] template resolution precedence: Explicit, builtin, known content-hash override, then embedded template; no usable template fails visibly.
- [2026-08-30-1-chat-templates-injection-defense] catalog subset profiles: Profiles select validated global chat, remote, and STT catalog entries rather than overriding fields.
- [2026-08-31-1-fix-hf-proxy-consistency] renderer-owned stderr: Native library diagnostics must enter structured logging instead of corrupting terminal progress rendering.
- [2026-08-31-2-rebuild-mdbook-from-includes] explicit fence semantics: Included operator examples declare their fence language so documentation tests do not interpret them as Rust.
- [2026-08-31-4-core-support-api-refinements] nonce observability: Guard nonces are correlatable identifiers, not secrets; construction remains controlled.
- [2026-08-31-5-desktop-shell-review-followup] transition-only window signals: Window state events should emit only when their semantic value changes.
- [2026-08-31-6-tauri-migration-for-workshop] desktop lifecycle ownership: The shell creates the gateway and window together and shuts the gateway down exactly once on app exit.
- [2026-08-31-7-interactive-webhook-tool] claim-once jobs: Future background work should buffer completions behind claim-once handles and one wait-any primitive.
- [2026-09-01-1-build-simplification] product release lanes: Gateway, Workshop, and native accelerator artifacts release independently on platform-appropriate lanes.
- [2026-09-02-1-workshop-agent-window-clone] reference-derived presentation: Visual parity values need source citations and measured layout composition, not token resemblance alone.
- [2026-09-01-2-nightly-installer-builds] fork-first workflow proof: Release workflow changes should prove their event and artifact paths on a fork before upstream scheduling.
- [2026-09-02-2-workshop-cursor-parity] single shortcut authority: When the app owns a shortcut, the embedded browser must not race it with a native accelerator.
- [2026-09-02-3-whisper-shared-lib] signed update chain: Desktop updates require signed platform artifacts and a signed manifest before install and relaunch.
- [2026-09-02-4-evergreen-links-and-version] evergreen alias: Stable download aliases may move only by repointing to already-versioned release artifacts.
- [2026-09-02-5-crate-taxonomy-rename] facade purity: The integrator-facing promptforge crate should expose documented re-exports and contain no implementation logic.
- [2026-09-02-6-product-user-guides] manifest-complete extraction: A documentation run visits every source in its lens and records empty extractions as coverage evidence.
- [2026-09-03-1-enhance-tts-endpoint-report] speech input fidelity: Preserve backend-significant inline speech tags in input; sanitizing them changes requested content.
- [2026-09-03-1-enhance-tts-endpoint-report] speech dialect edges: Voice objects, provider SSE, response formats, and transcoding remain adapter-level compatibility details.
- [2026-09-03-2-enhance-image-endpoint-spec] readiness means usable: A supervised inference child becomes ready only after a representative warm-up succeeds; pre-probe exit is failure regardless of exit code.
- [2026-09-03-2-enhance-image-endpoint-spec] image retry ambiguity: Preserve structured upstream codes and request IDs; never blindly retry generation that may already have completed and billed.
- [2026-09-03-3-gateway-sidecar-decomposition] shared-crate seam: Cross-product crates should each encode one dependency-light contract instead of growing a common junk drawer.
- [2026-09-04-1-async-boot-and-progress] superseding desired state: Repeated profile commands coalesce by target; a newer target cancels stale active work and alone may publish.
- [2026-09-04-1-async-boot-and-progress] progress surface split: Structured state drives visual progress while terminal output remains ordinary tracing lines.
- [2026-09-04-2-apply-as-queue-command] asymmetric command ordering: A broader Apply supersedes stale profile loading, while a later profile load waits behind Apply instead of discarding pending configuration.
- [2026-09-04-2-apply-as-queue-command] ambient loopback trust: Keyless admin access needs verified loopback peer identity plus browser fetch-metadata checks; explicit bad credentials still fail closed.
- [2026-09-04-3-unlock-inference-during-switches] transitional-state cleanup: A failed or cancelled spawn clears loading markers, tears down partial children, and leaves the surviving routing usable.
- [2026-09-04-3-unlock-inference-during-switches] bounded operational waits: Worker joins and idle artifact reads need finite bounds so cancellation and shutdown cannot hang indefinitely.
N1 | observation | Violates A2 @ crates/gateway-stt/tests/fixtures/realtime: not determinable from diff | Freeze the realtime transcription wire contract
N2 | observation | Violates A96 @ crates/workshop-server/ui/test/realtime-wire-fixtures.mjs: not determinable from diff | Freeze the realtime transcription wire contract; Record installed acceptance and repair UI fixture
N3 | observation | shared-parameter-cluster @ crates/gateway-stt-engine/src/engine.rs::SttEngine::transcribe_final: repeats samples, guidance, and finalized history across decode signatures | Move take ownership into gateway STT; Harden STT workers and extend release gates
N4 | observation | shared-parameter-cluster @ crates/gateway-stt-engine/src/final_pass.rs::FinalTranscriber::transcribe: repeats samples, guidance, and finalized history across decode signatures | Move take ownership into gateway STT
N5 | observation | shared-mutable-state @ crates/gateway-stt/src/take.rs::TakeState: shares mutex-protected take state between the socket and final pipeline tasks | Move take ownership into gateway STT; Bound Realtime session input ownership; Finalize realtime items independently; Replace the STT runtime with a speech facade; Publish generic speech discovery facts; Mount Gateway Realtime transcription
N6 | observation | oversized-unit @ crates/gateway-stt/src/take.rs: adds a 651-line take module | Move take ownership into gateway STT; Add bounded realtime audio ingestion; Bound Realtime session input ownership; Finalize realtime items independently; Replace the STT runtime with a speech facade; Publish generic speech discovery facts; Mount Gateway Realtime transcription; Make session retirement cleanup event-driven; Partition live hypotheses into disjoint fields; Schedule and rebase whole-window hypotheses
N7 | observation | oversized-unit @ crates/gateway-stt/tests/common/mod.rs: adds bounded shutdown logic to an already oversized test support module | Move take ownership into gateway STT
N8 | observation | oversized-unit @ crates/gateway-stt/tests/it/legacy_stream.rs: adds explicit shutdown calls to an already oversized integration suite | Move take ownership into gateway STT; Reconcile explicitly skipped final ranges
N9 | observation | Violates A2 @ crates/gateway-stt/src/runtime.rs: not determinable from diff | Move take ownership into gateway STT; Separate Whisper from the STT engine; Harden STT workers and extend release gates; Migrate STT tuning to canonical configuration; Add bounded realtime audio ingestion
N10 | observation | Violates A96 @ crates/gateway-stt/src/api.rs: not determinable from diff | Move take ownership into gateway STT; Harden STT workers and extend release gates
N11 | observation | flag-parameter @ crates/gateway-stt-backend-whisper/src/model.rs::WhisperDecoder::load: selects interim or final decode policy through final_pass | Separate Whisper from the STT engine
N12 | observation | flag-parameter @ crates/gateway-stt-engine/src/worker.rs::worker_loop: selects interim or final factory construction through final_model | Separate Whisper from the STT engine; Bound transcription workers and expose test fixtures; Harden STT workers and extend release gates
N13 | observation | global-state @ crates/gateway-stt-backend-whisper/src/prompt.rs::NATIVE_TEST: serializes fixture-dependent prompt tests with a process-wide mutex | Separate Whisper from the STT engine
N14 | observation | global-state @ crates/gateway-stt-backend-whisper/tests/native_whisper.rs::NATIVE_TEST: serializes native backend tests with a process-wide mutex | Separate Whisper from the STT engine
N15 | observation | clone-block @ crates/gateway-stt/src/test_fixtures.rs: duplicates native fixture loading across unit and integration test support | Separate Whisper from the STT engine; Quiesce speech generations before replacement; Centralize native STT fixture resolution
N16 | observation | clone-block @ crates/gateway-stt/tests/common/mod.rs: duplicates native fixture loading across integration and unit test support | Separate Whisper from the STT engine; Centralize native STT fixture resolution
N17 | observation | Violates A2 @ crates/gateway-stt/tests/it/architecture.rs: not determinable from diff | Enforce exact STT architecture ratchets; Finalize generic Realtime STT architecture; Centralize native STT fixture resolution; Ratchet feature-enabled STT fixture APIs
N18 | observation | feature-flag @ crates/gateway-stt-backend-whisper/Cargo.toml::test-fixtures: forwards scripted engine fixtures without an expiry | Bound transcription workers and expose test fixtures
N19 | observation | feature-flag @ crates/gateway-stt-engine/Cargo.toml::test-fixtures: gates downstream scripted decoder fixtures without an expiry | Bound transcription workers and expose test fixtures; Centralize native STT fixture resolution
N20 | observation | surface-growth @ crates/gateway-stt-engine/src/lib.rs::test_fixtures: exposes scripted decoder controls to downstream consumers | Bound transcription workers and expose test fixtures; Partition live hypotheses into disjoint fields; Centralize native STT fixture resolution
N21 | observation | shared-mutable-state @ crates/gateway-stt-engine/src/test_fixtures.rs::ScriptedDecoder: shares mutex-protected decoder controls across worker and test owners | Bound transcription workers and expose test fixtures; Harden STT workers and extend release gates; Make profile replacement transactional; Partition live hypotheses into disjoint fields; Narrow STT fixture controls to scenarios
N22 | observation | temporal-coupling @ crates/gateway-stt-engine/src/test_fixtures.rs::ScriptedDecoder: requires park, wait, and release calls in sequence | Bound transcription workers and expose test fixtures; Harden STT workers and extend release gates; Make profile replacement transactional; Partition live hypotheses into disjoint fields; Narrow STT fixture controls to scenarios
N23 | observation | feature-flag @ crates/gateway-stt/Cargo.toml::test-fixtures: gates the downstream speech test facade without an expiry | Bound transcription workers and expose test fixtures
N24 | observation | surface-growth @ crates/gateway-stt/src/lib.rs::test_fixtures: exposes scripted speech runtime construction to downstream consumers | Bound transcription workers and expose test fixtures; Bound Realtime session input ownership; Finalize realtime items independently; Replace the STT runtime with a speech facade; Quiesce speech generations before replacement; Publish generic speech discovery facts; Mount Gateway Realtime transcription; Make session retirement cleanup event-driven
N25 | observation | facade @ crates/gateway-stt/src/lib.rs::test_fixtures: combines engine fixtures with speech runtime construction | Bound transcription workers and expose test fixtures; Replace the STT runtime with a speech facade; Quiesce speech generations before replacement; Publish generic speech discovery facts; Mount Gateway Realtime transcription
N26 | observation | constructor-injection @ crates/gateway-stt/src/runtime.rs::SttRuntime::from_scripted_engine: receives the scripted engine and runtime settings as parameters | Bound transcription workers and expose test fixtures; Bound Realtime session input ownership; Replace the STT runtime with a speech facade
N27 | observation | Violates A115 @ crates/gateway-stt/src/runtime.rs: control readiness during model startup is not determinable from diff | Harden STT workers and extend release gates; Migrate STT tuning to canonical configuration
N28 | observation | Violates A116 @ crates/gateway/src/config_apply.rs::stt_pipeline_change_reloads_without_restart: publication consistency is not determinable from diff | Migrate STT tuning to canonical configuration
N29 | observation | Violates A96 @ crates/gateway-config-ui/ui/src/services/config-store.ts::canonicalizeStt: bounded third-party model content is not determinable from diff | Migrate STT tuning to canonical configuration
N30 | observation | Violates A2 @ crates/gateway-stt/src/realtime: not determinable from diff | Define the private Realtime wire; Bound Realtime session input ownership; Finalize realtime items independently; Mount Gateway Realtime transcription; Make session retirement cleanup event-driven; Partition live hypotheses into disjoint fields; Schedule and rebase whole-window hypotheses; Restore dead-code diagnostics for gateway STT
N31 | observation | global-state @ crates/gateway-stt/src/realtime/wire/shared.rs::NEXT_GENERATOR: allocates ID generator namespaces from a process-wide atomic counter | Define the private Realtime wire
N32 | observation | oversized-unit @ crates/gateway-stt/src/realtime/wire/server.rs::ServerEvent::validate: adds an 85-line server event validator | Define the private Realtime wire
N33 | observation | flag-parameter @ crates/gateway-stt/src/realtime/wire/client.rs::parse_empty: selects commit or clear event construction through commit | Define the private Realtime wire
N34 | observation | clone-block @ crates/gateway-stt/tests/it/realtime_session.rs: repeats session update, audio encoding, snapshot, clear, epoch, and capacity cases from unit tests | Bound Realtime session input ownership; Finalize realtime items independently; Make session retirement cleanup event-driven
N35 | observation | hidden-dependency @ crates/gateway-stt/src/generation.rs::unload: waits for generation and engine reference counts outside its interface | Replace the STT runtime with a speech facade; Quiesce speech generations before replacement
N36 | observation | Violates A2 @ crates/gateway-stt/src/service.rs::SpeechService: credential ownership is not determinable from diff | Replace the STT runtime with a speech facade; Make profile replacement transactional; Publish generic speech discovery facts; Retire legacy speech seams
N37 | observation | Violates A115 @ crates/gateway/src/runner.rs::Gateway::from_config_with_hub: control readiness during speech provisioning is not determinable from diff | Replace the STT runtime with a speech facade; Make profile replacement transactional
N38 | observation | shared-parameter-cluster @ crates/gateway-stt/src/generation/snapshot.rs::Generation::from_factory: repeats id, backend, names, and guidance across generation constructors | Quiesce speech generations before replacement; Make profile replacement transactional
N39 | observation | global-state @ crates/gateway/src/config_write.rs::PERSISTENCE_SEQUENCE: allocates persistence temporary suffixes from a process-wide atomic counter | Make profile replacement transactional; Harden prepared persistence names; Extract profile switch preparation phases
N40 | observation | hidden-dependency @ crates/gateway/src/config_write.rs::persistence_temporary: reads process identity and a global sequence outside its interface | Make profile replacement transactional; Harden prepared persistence names
N41 | observation | shared-parameter-cluster @ crates/gateway/src/lib.rs::prepare_cutover: repeats state, profile, and cancellation across switch phase functions | Make profile replacement transactional; Extract profile switch preparation phases
N42 | observation | shared-parameter-cluster @ crates/gateway/src/lib.rs::run_switch_phases: repeats state, profile, and cancellation across switch phase functions | Make profile replacement transactional; Extract profile switch preparation phases
N43 | observation | shared-parameter-cluster @ crates/gateway/src/lib.rs::commit_switch: repeats state, profile, and cancellation across switch phase functions | Make profile replacement transactional; Complete the profile-switch transaction
N44 | observation | shared-parameter-cluster @ crates/gateway/src/lib.rs::restore_or_shutdown: repeats state, cancellation, and failure across rollback functions | Make profile replacement transactional; Extract profile switch preparation phases
N45 | observation | shared-parameter-cluster @ crates/gateway/src/lib.rs::request_fatal_shutdown: repeats state, cancellation, and failure across rollback functions | Make profile replacement transactional; Complete the profile-switch transaction
N46 | observation | shared-parameter-cluster @ crates/gateway/src/lib.rs::rollback_commit_failure: repeats state, cancellation, and failure across rollback functions | Make profile replacement transactional; Complete the profile-switch transaction
N47 | observation | flag-parameter @ crates/workshop-server/src/gateway/socket.rs::GatewayClient::connect_socket: selects the legacy status header through workshop_status | Add the Workshop Realtime relay; Retire legacy speech seams
N48 | observation | shared-mutable-state @ crates/workshop-server/tests/it/realtime_relay.rs::UpstreamProbe: shares mutex-protected request and frame observations across relay and test owners | Add the Workshop Realtime relay
N49 | observation | oversized-unit @ crates/workshop-server/tests/it/realtime_relay.rs::upstream: adds a 79-line upstream probe handler | Add the Workshop Realtime relay; Require a model before built-in chat turns; Converge chat sessions with live catalogs; Split Workshop relay integration coverage
N50 | observation | oversized-unit @ crates/workshop-server/tests/it/realtime_relay.rs::realtime_relay_is_authenticated_fixed_and_payload_opaque: adds an 83-line wire behavior test | Add the Workshop Realtime relay; Require a model before built-in chat turns; Converge chat sessions with live catalogs; Split Workshop relay integration coverage
N51 | observation | shared-mutable-state @ crates/workshop-server/tests/it/realtime_relay.rs::StalledPeerProbe: shares frame delivery state between peer and test owners | Add the Workshop Realtime relay
N52 | observation | shared-mutable-state @ crates/workshop-server/ui/src/main.ts::speechCapture: shares one mutable microphone capture service across agent panels | Migrate Workshop dictation to Realtime
N53 | observation | surface-growth @ crates/workshop-server/ui/src/services/realtime-transcription.ts: exports the browser Realtime socket, event, and options contract | Migrate Workshop dictation to Realtime; Converge Workshop startup state; Decode Realtime events exhaustively
N54 | observation | event-hook @ crates/workshop-server/ui/src/services/realtime-transcription.ts::RealtimeTranscriptionService: exposes transcription state and item outcomes through six callback events | Migrate Workshop dictation to Realtime; Converge Workshop startup state
N55 | observation | dispatch-on-tag @ crates/workshop-server/ui/src/services/realtime-transcription.ts::handleMessage: routes server events through one string-tag branch chain | Migrate Workshop dictation to Realtime; Converge Workshop startup state; Decode Realtime events exhaustively
N56 | observation | shared-parameter-cluster @ crates/workshop-server/ui/src/ui/realtime-stt.ts::setupStt: repeats elements, status, and blocker across Realtime and legacy setup signatures | Migrate Workshop dictation to Realtime
N57 | observation | surface-growth @ crates/workshop-server/ui/src/ui/stt.ts::SttInputTarget: requires consumers to expose selected-range text for rollback | Migrate Workshop dictation to Realtime; Require a model before built-in chat turns; Recover Workshop after local Gateway exits
N58 | observation | constructor-injection @ crates/workshop-server/ui/src/ui/agent-session-view.ts::AgentSessionView: receives shared microphone capture through the panel constructor chain | Migrate Workshop dictation to Realtime; Require a model before built-in chat turns
N59 | observation | Violates A96 @ crates/workshop-server/ui/src/ui/realtime-stt.ts::setupStt: bounded third-party model content is not determinable from diff | Bind live hypotheses before commit acknowledgment
N60 | observation | Violates A2 @ crates/gateway-stt/src/take: credential ownership is not determinable from diff | Reconcile explicitly skipped final ranges
N61 | observation | oversized-unit @ crates/gateway-logging/src/queue.rs::byte_blocked_producers_wake_after_drain_and_close: adds a 98-line byte-blocked producer concurrency test | Order and bound logging queue admission; Bound logging stalls and shutdown
N62 | observation | Violates A2 @ crates/gateway-logging/src/queue.rs: credential ownership in gateway logging is not determinable from diff | Order and bound logging queue admission; Bound logging stalls and shutdown; Redact logging fields before formatting
N63 | observation | newtype @ crates/gateway-logging/src/worker.rs::LogWorker: owns the worker join handle for bounded completion or detachment | Bound logging stalls and shutdown
N64 | observation | oversized-unit @ crates/gateway-logging/src/queue.rs::LogQueue::enqueue_after: expands producer admission and timeout handling to 83 lines | Bound logging stalls and shutdown
N65 | observation | oversized-unit @ crates/gateway-logging/src/runtime.rs::assert_stalled_shutdown: adds a 97-line deterministic stalled-shutdown test helper | Bound logging stalls and shutdown
N66 | observation | shared-parameter-cluster @ crates/gateway-logging/src/queue.rs::LogQueue::new_for_test_with_wait: repeats max_records, max_bytes, and producer_wait across queue constructors | Bound logging stalls and shutdown
N67 | observation | flag-parameter @ crates/gateway-logging/src/queue.rs::LogQueue::complete_batch: uses had_summary to select summary completion accounting | Bound logging stalls and shutdown
N68 | observation | Violates A2 @ crates/gateway-logging/src/worker.rs: credential ownership in gateway logging is not determinable from diff | Rotate logs within fixed byte budgets; Redact logging fields before formatting
N69 | observation | oversized-unit @ crates/gateway-stt-engine/tests/feature_boundary.rs: adds a 179-line feature boundary integration test | Centralize native STT fixture resolution
N70 | observation | Violates A2 @ crates/gateway-stt-engine/src/test_fixtures: credential ownership in Gateway speech fixture changes is not determinable from diff | Narrow STT fixture controls to scenarios
N71 | observation | oversized-unit @ tools/check-stt-architecture.mjs::maskRustCommentsAndLiterals: adds a 97-line comment and literal masking function | Restore dead-code diagnostics for gateway STT
N72 | observation | Violates A116 @ crates/gateway/src/config_write.rs::PreparedFile: publication consistency with live state is not determinable from diff | Harden prepared persistence names
N73 | observation | Violates A117 @ crates/gateway/src/config_write.rs::PreparedFile: routing availability during switch preparation is not determinable from diff | Harden prepared persistence names
N74 | observation | Violates A2 @ crates/gateway/src/profile_switch.rs: credential ownership in the profile-switch transaction is not determinable from diff | Complete the profile-switch transaction
N75 | observation | oversized-unit @ crates/workshop-server/ui/src/services/realtime-event-decoder.ts::decodeRealtimeEvent: adds a 144-line exhaustive event decoder | Decode Realtime events exhaustively
N76 | observation | Violates A96 @ crates/workshop-server/ui/src/services/realtime-event-decoder.ts: bounded third-party model content is not determinable from diff | Decode Realtime events exhaustively
