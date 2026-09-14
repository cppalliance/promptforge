---
name: section-lua-lifecycle
overview: Refactor PromptForge sections into shared compiled Lua, semantic capability binding, preamble, prose, and epilog phases with isolated per-section VMs. Add a new authoritative `design-core.md` that explains the complete crate design, rationale, invariants, and supported features while retaining `design-core-orig.md` as history.
todos:
  - id: observer-seam
    content: Replace structured events with report-only observations
    status: completed
  - id: tool-identity
    content: Establish stable live-tool identity
    status: completed
  - id: picker-overlap
    content: Add picker near-duplicate analysis
    status: completed
  - id: lua-program
    content: Compile Lua source into reusable programs
    status: completed
  - id: section-vm
    content: Preserve one isolated VM through each section
    status: completed
  - id: lua-tool-modes
    content: Add need, always, and add phase APIs
    status: completed
  - id: h1-grammar
    content: Require H1 and parse shared prompt Lua
    status: completed
  - id: section-grammar
    content: Parse preamble, prose, and epilog
    status: completed
  - id: bind-needs
    content: Bind capability aliases through the picker
    status: completed
  - id: binding-uniqueness
    content: Enforce one-to-one alias and tool identity
    status: completed
  - id: lifecycle-execution
    content: Execute the complete section lifecycle
    status: completed
  - id: alias-dispatch
    content: Scope and dispatch aliased tools
    status: completed
  - id: cli-integration
    content: Integrate the CLI live tool registry
    status: completed
  - id: mcp-integration
    content: Integrate the MCP server live tool registry
    status: completed
  - id: migrate-document
    content: Migrate prompts and finalize authoritative design
    status: completed
isProject: false
---

# Refactor the Section Lua Lifecycle

## Resolution 1: What is being built

PromptForge will parse one required H1, one optional shared `lua prompt` block immediately after it, and H2 sections containing an optional leading Lua preamble, prose, and optional trailing Lua epilog. Parsing compiles every Lua region once into source-backed process-local bytecode. Launch binding resolves prompt-local capability aliases to stable live `ToolId` values through `promptforge-tool-picker`. Execution creates one isolated VM per section, preserves its globals and functions across the model turn, exposes the final model text as `reply`, runs the epilog, and destroys the VM. One report-only observer receives deterministic `(Section, Detail)` strings across parsing, binding, execution, Lua phases, scope validation, model and tool boundaries, and harness-mediated store operations.

The new [`design-core.md`](C:/Users/Vinnie/src/cursor/promptforge/crates/promptforge-core/design-core.md) is the authoritative design and is updated with each behavior-bearing step. [`design-core-orig.md`](C:/Users/Vinnie/src/cursor/promptforge/crates/promptforge-core/design-core-orig.md) remains byte-for-byte historical. Fan-out execution, branching, retries, child execution, persistent bytecode, compatibility parsing, reranking, and model-generated progress labels are non-goals.

Two rules govern every choice:

1. Use the smallest existing facility that can carry the requirement.
2. Provide one way to perform an operation unless a documented constraint requires another.

## Resolution 2: Components in dependency order

1. **Report-only observer contract.** Every later component reports through one borrowed string-pair seam, so the seam precedes all instrumented behavior.
2. **Tool identity contract.** Live tools expose stable identity, description, and schema. Capability binding and dispatch depend on this identity.
3. **Picker overlap analysis.** The picker reports near-duplicate selected tools using its existing vectors and threshold. Effective scope validation depends on this API.
4. **Compiled Lua runtime.** `LuaProgram` owns source and bytecode; `SectionVm` owns one sandbox across both section phases. Parser output and execution depend on these types.
5. **Prompt grammar.** The parser extracts H1 shared Lua and three-phase sections, then returns only syntax-valid executable prompts.
6. **Lua capability phases.** H1 declares needs and universal aliases; H2 scopes aliases. Binding and execution depend on phase-correct host APIs.
7. **Capability binding.** H1 needs become an immutable one-to-one alias and `ToolId` map before execution.
8. **Section executor.** The executor consumes a bound prompt, validates effective scope, runs the model, and dispatches aliased calls.
9. **Host adapters.** CLI and MCP construct complete matching live registries and picker catalogs.
10. **Prompt migration and public design.** Repository prompts and public documentation adopt the final grammar and semantics.

## Resolution 3: Settled component behavior

### Report-only observation

- Replace `Event` and `Observer::on_event` with the sole operation `Observer::observe(section: &str, detail: &str)`.
- `section` is the H2 heading for section activity and the required H1 title for prompt-wide activity after parsing. Before the H1 is known, use the fixed string `Prompt`.
- `detail` is a concise deterministic operational statement. It may contain fixed metadata such as alias, `ToolId`, score, turn count, elapsed time, and success state, but never raw prompt prose, arguments, model input or output, tool arguments or results, store contents or paths, credentials, or fetched content.
- Every observing API receives an always-present `&dyn Observer`; silence uses `NullObserver`, never `Option` or a second convenience API.
- Observation is synchronous, non-blocking, report-only, and never consulted for a decision. Recording and null observers must produce identical outputs, errors, ordering, and side effects.
- The same observer is threaded through parse, Lua compilation, binding, execution, `SectionVm`, effective-scope validation, model and tool boundaries, and store calls made through the execution harness. Direct `Store` use outside the harness is unobserved.
- MCP may recognize a small documented set of exact details for cosmetic numeric progress. Unknown details are logged or ignored and never affect execution.
- The original `(Section, Detail)` pair is the trace record. No model-generated text rewrites or replaces it.

### Grammar and compilation

- YAML frontmatter no longer has `tools`.
- Ordinary Markdown between frontmatter and the required H1 is ignored.
- Blank lines may separate H1 from `lua prompt`; any other content before that reserved fence makes the fence misplaced and parsing fails.
- Shared Lua uses an exact unindented triple-backtick opening line `lua prompt` and exact triple-backtick closing line. Section phases use the same exact form tagged `lua`. Longer markers, indentation, and extra info tokens remain prose.
- A section has optional leading preamble, middle prose, and optional trailing epilog. Other and middle fences remain prose.
- `Prompt::parse` retains Lua source for diagnostics and compiles process-local Lua 5.4 bytecode. Bytecode is never persisted.
- Accepted top-level returns are string, integer, number, and boolean, rendered as the existing string result. Nil continues. Tables and unsupported values are errors.

### Lua isolation and lifecycle

- Shared source is compiled once but executed independently in every section VM.
- One VM remains alive from shared load through preamble, model await, `reply` binding, and epilog.
- Shared functions can read host globals injected after shared load because they retain the same environment table.
- H1 declarations replay only from immutable cached bindings and perform no embeddings.
- `tools.need` and `tools.always` are legal only during top-level H1 execution. Calling them later through a helper is an error.
- `tools.add` is legal only during H2 preamble execution and closes before any model turn.
- Near-duplicate effective-scope validation runs only when a model turn will occur. A preamble return ends the run before that validation.
- A preamble scalar return emits `SectionFinished`, skips prose, model, and epilog, then ends the run. An epilog scalar return follows model events, emits `SectionFinished`, and ends the run.
- No Lua memory crosses sections. `Store` remains the sole intentional cross-section mutable channel.

### Semantic capability binding

- H1 declares `tools.need(alias, description)` with a unique model-facing alias and a parameter-free author-register capability sentence.
- The alias must match `[A-Za-z][A-Za-z0-9_-]{0,63}` and is case-sensitive.
- Tools are never injected automatically. H2 `tools.add(alias)` is the sole section-scoping operation. H1 `tools.always(alias)` is an explicit prompt-wide scope for genuinely universal capabilities.
- The existing picker uses BGE embeddings plus floor, margin, duplicate, and ambiguity policy. It has no reranker.
- `Bind` creates the alias-to-ID entry. `Absent`, `Duplicate`, and `Ambiguous` fail before execution begins; no shortlist, fallback, or model adjudication exists.
- Binding is one-to-one: each alias maps to exactly one `ToolId`, each `ToolId` appears under at most one alias, and duplicate live IDs fail.
- The model sees each selected concrete tool under its local alias with the concrete description and schema. Returned alias calls dispatch through the bound `ToolId`.
- Before a model turn, the picker checks every pair in the effective `tools.always` plus H2 `tools.add` set. A pair at or above `duplicate_threshold` is an error with aliases, IDs, descriptions, hints, and score. Similar tools isolated in different sections are valid.
- Launch records the complete alias-to-ID map for diagnostics.

### Public API and async boundary

- `LuaProgram`, parsed `Prompt`, and immutable `BoundPrompt` are public because hosts parse, bind, and execute in separate phases. Each phase requires the same observer reference.
- Binding is a synchronous public operation over a prepared picker and live registry. Async hosts run it in `spawn_blocking`; the CLI calls it directly.
- `execute::run` accepts a `BoundPrompt` and preserves existing store, observer, child-section, and fall-through behavior.
- mlua's `send` feature keeps the execution future `Send`; no mutex guard may cross an await.

```mermaid
flowchart LR
    Source[MarkdownSource] --> Parse[ParseAndCompile]
    Parse --> Prompt[ExecutablePrompt]
    Prompt --> Resolve[ResolveH1Needs]
    Resolve --> Bound[BoundPrompt]
    Bound --> Vm[CreateSectionVm]
    Vm --> Shared[ReplaySharedBytecode]
    Shared --> Preamble[RunPreamble]
    Preamble --> Scope[ValidateEffectiveScope]
    Scope --> Model[SubstituteAndRunModel]
    Model --> Reply[BindReply]
    Reply --> Epilog[RunEpilog]
    Epilog --> Drop[DestroySectionVm]
```

## Resolution 4: Testable commit steps

Each step carries its code, owning regression tests, Rust documentation, and the relevant update to `design-core.md`, README, or STATUS. Run the targeted test first, then the full verification suite before the commit gate. These are planned commit boundaries, but no commit may be created until the user explicitly authorizes commit creation.

1. **Report-only observer seam**
   - Code: remove the structured `Event` enum, serialization, cloning, and variant matching; replace the trait with `observe(section, detail)`; retain `Send + Sync`, synchronous reporting, and `NullObserver`; thread the observer through existing execution, model, tool, and harness-mediated store boundaries; adapt MCP progress to a small pinned detail vocabulary.
   - Test: trait compatibility, null behavior, deterministic ordering, exact safe strings, failure reports, payload exclusion, execution equivalence between recording and null observers, and MCP handling of recognized and unknown details.
   - Complete when `Event` and its wire format no longer exist and observer choice cannot change execution behavior.
2. **Stable live-tool identity**
   - Code: extend [`tools.rs`](C:/Users/Vinnie/src/cursor/promptforge/crates/promptforge-core/src/tools.rs) and concrete tools with stable `ToolId`, description, and schema fidelity; preserve concrete wire names only as transport details.
   - Test: descriptor fidelity and ID-based registry lookup.
   - Complete when current hosts compile against the identity surface without semantic binding.
3. **Picker near-duplicate primitive**
   - Code: add public `NearDuplicate` and `ToolPicker::near_duplicates(ids)` in [`promptforge-tool-picker`](C:/Users/Vinnie/src/cursor/promptforge/crates/promptforge-tool-picker), reusing stored vectors and `duplicate_threshold`.
   - Test: absent and repeated IDs, below threshold, exact threshold, cross-server pairs, and deterministic order.
   - Complete when picker tests pass without a core dependency.
4. **Parse-time `LuaProgram`**
   - Code: add source retention, compilation, bytecode dump and load, a location-bearing compilation error, and safe parse and compile reports in [`lua.rs`](C:/Users/Vinnie/src/cursor/promptforge/crates/promptforge-core/src/lua.rs) and [`error.rs`](C:/Users/Vinnie/src/cursor/promptforge/crates/promptforge-core/src/error.rs).
   - Test: bytecode round trip, malformed Lua, and source-bearing diagnostics.
   - Complete when parser and executor remain otherwise unchanged.
5. **Persistent `SectionVm`**
   - Code: add hardened VM ownership, one environment, delayed host injection, preamble, `reply`, epilog, scalar return extraction, one instruction counter, mlua `send`, send-safe recorders, and reports for shared load, preamble, reply binding, epilog, and teardown.
   - Test: helper visibility, two-VM isolation, reply visibility, instruction-budget continuity, return conversion, and compile-time `Send`.
   - Complete without capability APIs or model execution.
6. **Lua declaration and scope modes**
   - Code: add binding, replay, and H2 recording modes for `tools.need`, `tools.always`, and `tools.add`, reporting phase outcomes without need descriptions.
   - Test: alias validation, duplicate declarations, unknown aliases, empty add, phase restrictions, scope closure, and replay mismatch.
   - Complete using a deterministic resolver seam without the full picker binder.
7. **Required H1 and shared-library grammar**
   - Code: remove `Frontmatter::tools`, require H1, make title non-optional, ignore preface, reserve the immediately leading `lua prompt`, compile it, and report parse boundaries without source text.
   - Test: missing H1, ignored preface, blank-line allowance, valid immediate library, misplaced or duplicate libraries, exact fence rules, and malformed shared Lua.
   - Complete while existing section parsing remains intact.
8. **Three-phase section grammar**
   - Code: replace `Section::lua` with compiled preamble and epilog around prose; update substitution documentation and safe phase reports.
   - Test: preamble-only, epilog-only, all phases, empty prose, middle fences as prose, exact fence rules, and location-bearing phase errors.
   - Complete when parser tests pass and only intentionally unmigrated prompts fail.
9. **Four-outcome capability binding**
   - Code: add the core picker dependency, public immutable `BoundPrompt`, one synchronous H1 binding pass, exact replay cache, diagnostics map, distinct outcome errors, and binding outcome reports without descriptions or candidate text.
   - Test: Bind, Absent, Duplicate, Ambiguous, candidate-rich diagnostics, exact caching, and zero replay embeddings.
   - Complete when immutable mappings are produced but collision hardening remains isolated.
10. **Binding uniqueness and registry agreement**
   - Code: build alias-to-ID and ID-to-alias maps atomically; reject duplicate live IDs, aliases, two aliases selecting one ID, and picked IDs absent from the live registry; report validation outcomes without registry payloads.
   - Test: every collision and catalog mismatch, with no partially valid `BoundPrompt`.
   - Complete when every identity boundary fails loudly.
11. **Tool-free section lifecycle execution**
    - Code: execute `BoundPrompt` through shared load, preamble, model, reply, epilog, observer reports, store, and fall-through without tool advertisement.
    - Test: preamble return, epilog return, helper continuity, empty prose, report ordering, null-observer equivalence, store persistence, and fall-through precedence.
    - Complete when existing outer contracts pass.
12. **Aliased tool scope and dispatch**
    - Code: combine `always` and `add`, validate effective IDs through the picker, advertise aliases, dispatch aliases to `ToolId`, and report scope, model, and tool boundaries without arguments, replies, results, or store data.
    - Test: no automatic injection, `always`, H2 `add`, aliased schemas, concrete dispatch, similar tools isolated in separate sections, and near-duplicate failure before model.
    - Complete when prompt code requires no canonical tool name.
13. **CLI registry integration**
    - Code: construct the complete live registry and matching picker catalog in [`promptforge-cli`](C:/Users/Vinnie/src/cursor/promptforge/crates/promptforge-cli); omit unavailable gateway tools, bind synchronously, and pass `NullObserver` unless the caller installs another observer.
    - Test: available binding, absent unavailable capability, and ID agreement.
    - Complete when CLI selection uses needs only.
14. **MCP registry integration**
    - Code: construct or share the prepared picker and complete live registry in [`promptforge-mcp-server`](C:/Users/Vinnie/src/cursor/promptforge/crates/promptforge-mcp-server); move binding to `spawn_blocking`; consume reports and recognize only pinned cosmetic details.
    - Test: registry agreement, semantic binding, `Send` execution, pinned progress details, and unknown-detail tolerance.
    - Complete when CLI and MCP use the same core contract.
15. **Repository migration and authoritative design**
    - Code: add the missing H1 to [`research-person.md`](C:/Users/Vinnie/src/cursor/promptforge/prompts/research-person.md), replace YAML tool names with aliased needs, and use `tools.always` only where universal.
    - Test: every repository prompt parses and binds under the new grammar.
    - Docs: finish `design-core.md`, README, and STATUS; document the spike's domain-dependent reranker results; leave `design-core-orig.md` unchanged.
    - Complete when the full workspace verification suite passes and no documentation describes the old grammar.

## Decision log and falsifiers

1. **One report-only observer seam.** Every phase reports through the same borrowed `(Section, Detail)` pair; `NullObserver` provides silence and reports never influence behavior. Falsifier: a required correctness consumer needs typed observer data, or a required diagnostic cannot be expressed safely and deterministically.
2. **Required H1 and ignored preface.** H1 is the semantic prompt boundary. Falsifier: repository content requires pre-H1 text to affect execution.
3. **One immediately leading `lua prompt`.** Executable shared code must be unambiguous. Falsifier: a valid authoring case requires shared code after description prose.
4. **Exact leading preamble and trailing epilog fences.** Ordinary examples must remain prose. Falsifier: exact extraction cannot preserve real CommonMark prompt content.
5. **Parse-time compilation.** A successful `Prompt` must be executable. Falsifier: dumped bytecode cannot safely retain environment behavior or diagnostics for the prompt lifetime.
6. **Shared source with isolated execution.** Helper reuse must not create cross-section mutable Lua state. Falsifier: a settled feature requires shared Lua closures across sections.
7. **Semantic local aliases only.** Prompts state needs while hosts own concrete availability. Falsifier: a required capability cannot be described accurately without naming an implementation.
8. **Explicit `add` and `always`.** No capability is exposed accidentally. Falsifier: universal capabilities cannot be expressed without repetitive section code.
9. **One binding pass and exact replay.** Replay is deterministic and performs no embeddings. Falsifier: tool availability must intentionally change during one run.
10. **One-to-one alias and identity.** Symbols and dispatch targets must be unambiguous. Falsifier: a supported host requires intentional synonyms for one `ToolId`.
11. **Near-duplicate validation per effective scope.** Only tools competing in one model turn need semantic separation. Falsifier: the calibrated threshold rejects distinct operations with adequate descriptions and hints.
12. **One persistent VM per section.** Preamble functions and globals must remain visible to epilog. Falsifier: the VM cannot remain safe and `Send` across model await.
13. **Scalar return ends the run.** This preserves compact Lua control flow. Falsifier: an accepted scalar cannot map honestly to the string result contract.
14. **Store is the sole cross-section mutable channel.** Explicit state boundaries enable future fan-out. Falsifier: a settled feature requires run-global Lua memory.
15. **No reranker or compatibility parser.** Neither is justified by current measurements or release state. Falsifier: author-register measurements validate a reranker, or released prompts require compatibility.
16. **No label model in this refactor.** Raw trace pairs remain authoritative; model-generated labels are a separate optional UI extension. Falsifier: an accepted run-specific progress feature and measured quality and latency justify the label pipeline without weakening trace fidelity or privacy.

## Data-flow and gap audit

`Markdown source + Observer -> parser and Lua compiler -> executable Prompt -> live registry plus picker -> BoundPrompt -> section VM -> effective alias scope -> aliased model call -> ToolId dispatch -> reply -> epilog -> existing string result`

- Every consumer receives a typed output from its predecessor.
- The same observer reference travels alongside the typed data flow and emits `(Section, Detail)` only as a side channel.
- Reports never flow back into parsing, binding, execution, dispatch, store behavior, or return selection.
- `NullObserver` discards the side channel with identical behavior; no model sees or rewrites trace details.
- Source compilation occurs once; shared bytecode is instantiated independently per VM.
- Embeddings occur only during initial binding.
- Alias and ID maps validate before execution; effective scope validates before model exposure.
- Expected failures are introduced with the behavior that creates them, not in detached error commits.
- Tests and documentation move with each behavior.
- The plan is self-contained and no longer depends on chat-only decisions.

## Parallelism

- Steps 1, 2, and 3 may run in parallel on isolated branches or worktrees.
- Step 4 follows step 1.
- Step 5 follows step 4.
- Step 6 follows steps 1, 2, and 5.
- Step 7 follows step 4 and may overlap step 5.
- Step 8 follows steps 1, 5, and 7.
- Step 9 follows steps 1, 2, 3, 6, and 7.
- Step 10 follows step 9.
- Step 11 follows steps 1, 8, and 10.
- Step 12 follows steps 1, 3, 6, and 11.
- Steps 13 and 14 may run in parallel after step 12.
- Step 15 follows both host integrations.
- Do not begin dependent same-branch work while a prior commit is under review because its fixes must fold into that commit.

## Verification commands

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --locked --workspace --all-features`
- `cargo test --doc`
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`

## Project-specific review

<project-review>
1. Does the change use the smallest existing PromptForge facility that can carry the requirement?
2. If machinery was added, does the commit documentation explain why existing facilities were insufficient?
3. Does the diff implement only the named step?
4. Are expected failures returned without panic, unwrap, expect, or swallowed errors?
5. Is unsafe code absent?
6. Are public items and error conditions documented?
7. Are affected design, README, and STATUS claims current?
8. Is `design-core-orig.md` unchanged?
9. Are prompts and frontmatter free of concrete tool-name dependencies?
10. Can H1 replay run without embeddings or mutable binding decisions?
11. Are alias-to-ID and ID-to-alias mappings unambiguous?
12. Is `Store` still the sole intentional cross-section mutable channel?
13. Is no mutex guard held across an await, and does the execution future remain `Send`?
14. Is `Event` absent and is `observe(section, detail)` the only observer operation?
15. Are reports deterministic, concise, and free of raw prompts, inputs, outputs, tool data, store data, paths, credentials, and fetched content?
16. Does `NullObserver` produce behavior identical to a recording observer?
17. Are parse, bind, execute, Lua, scope, model, tool, and harness-store boundaries observed?
18. Does MCP treat recognized strings as cosmetic only and tolerate unknown details without affecting execution?
19. Is the label model absent and is the original trace pair preserved?
20. Are store behavior, child behavior, and fall-through preserved?
21. Does every new behavior have a regression test that fails without it?
22. Do formatting, Clippy, workspace tests, doctests, and warning-denied documentation pass?
</project-review>

## Vibe execution protocol

1. The main context selects one numbered step, previews exactly what the implementer will receive, and dispatches an implementation subagent with this plan path and step number.
2. The implementer reads this plan, [`rust-how-to.md`](C:/Users/Vinnie/src/cursor/tools-public/how-to/rust-how-to.md), and repository rules; implements only that step; adds its tests and documentation; and runs targeted checks.
3. The main context performs only bounded status and verification checks. If commit creation has not been explicitly authorized, it stops with a verified working tree.
4. After explicit authorization, the main context stages and creates the step commit.
5. A fresh reviewer reads the commit diff, the general review block in [`vibe-how-to.md`](C:/Users/Vinnie/src/cursor/tools-public/how-to/vibe-how-to.md), and `<project-review>`. It overwrites scratch `vibe-review.md` with actionable failures only.
6. A fresh fixer reads the numbered step and scratch review, applies only listed corrections, and reruns checks.
7. Review and fix repeat until the scratch review is empty. Amend only when the commit was created in this session, remains unpushed, and amendment is authorized.
8. A bug introduced by an earlier completed commit gets its own explicitly authorized fix commit.
9. After ten failed code-and-test attempts, dispatch external research for prior art or evidence that the approach should change. Ask the user only if the resulting decision is hard to reverse.

Confidence: high - the revised plan is self-contained, dependency-ordered, testable one commit at a time, reviewable in fresh contexts, and explicit about its remaining falsifiers.