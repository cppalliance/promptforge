# Agent Runtime Architecture: Field Comparison and Adoption Priorities

Report type: evaluation / review. It judges PromptForge against five open-source agent runtimes sharing its tool-loop and hybrid-orchestration technique, then prescribes idioms to adopt in payoff order.

## Executive summary

PromptForge is not inventing agent loops, but its exact combination of rendered Markdown prose and editable Lua orchestration remains unusual and defensible. Tactus comes closest to the proposed callable Lua episode model; IronCrew comes closest to the Rust and `mlua` stack; Leeway most clearly demonstrates deterministic nodes wrapped around autonomous loops; Goose supplies the mature context, elicitation, and recovery machinery; agent-runtime provides a small readable version of workflow-agent composition. PromptForge already beats all five on typed Markdown prose and direct author control, but it trails the field on automatic compaction, complete tool-history projection, resumable episode identity, and generic blocking input. The highest-payoff move is to make every model-facing section one callable agent context while keeping Lua in charge of when prose becomes a system message, user message, one model round, or a complete tool loop.

### Key findings

1. **Adopt Tactus and IronCrew's callable episode boundary.** Four references independently place autonomous model loops inside deterministic orchestration, while PromptForge still exposes pipeline and agent executors as siblings that cannot compose. Confidence: high.
2. **Steal Goose's dual-visibility compaction model.** Keep the complete event log, replace only the model projection, preserve system and pinned context, and compact tool exchanges atomically. Confidence: high.
3. **Centralize tool-protocol healing before every provider call.** Goose, IronCrew, Tactus, and agent-runtime all repair or reject incomplete assistant-call and tool-result groups at one boundary. Confidence: high.
4. **Promote Workshop input into a generic elicitation protocol.** The field treats human input as a durable host-owned wait usable by deterministic Lua and autonomous model tools. Confidence: high.
5. **Bind system, tools, model, and compaction policy into episode identity.** IronCrew and Tactus show how to reject silent resume under changed behavior. Confidence: high.
6. **Return autonomous decisions through typed signals.** Leeway and Goose keep model judgment inside a deterministic outer graph by validating structured episode outcomes. Confidence: high.
7. **Persist reconstructible effects rather than opaque executor stacks.** Goose, Tactus, and agent-runtime demonstrate restart from durable boundaries. Confidence: medium.
8. **Split orchestration centers before adding these mechanisms.** Every reference that deferred decomposition accumulated giant modules or parallel engines. Confidence: high.

## Method

The study first profiled PromptForge through nine architecture lenses and named eight deficits and five strengths. A field survey checked sixteen candidates against source and shortlisted five by technique fit, using popularity only as a tiebreaker. Five dives inspected pinned tip revisions, and four provenance examinations traced cited idioms through repository history; Tactus remained tip-only at the operator's direction. A final citation check opened every cited location in the pinned clones and verified 44 of 46 on the first pass; both failures were incomplete ranges and were corrected before this report.

## Reference projects and provenance

**Tactus.** 2 stars. Chosen as the closest Lua model for explicit model calls, stateful agents, tools, human interaction, and child procedures inside imperative control. MIT. Provenance unknown by operator instruction; tip source only.

**IronCrew.** 2 stars. Chosen as the closest Rust 2024, Tokio, `mlua`, blocking-input, subflow, and tool-loop stack. MIT. Four cited idioms carry strong human signals and two carry explicit AI markers.

**Leeway.** 113 stars. Chosen as the clearest deterministic graph around bounded interactive or autonomous agent nodes. MIT. Five cited idioms carry explicit AI markers and have no earlier form.

**Goose.** 53,962 stars. Chosen as the mature Rust implementation of compaction, session effects, tool healing, recipes, and elicitation. Apache-2.0. Six cited idioms carry explicit AI markers; available rewinds retained or tightened the mechanisms.

**agent-runtime.** 5 stars. Chosen as the small readable Rust implementation of agent steps, workflows, checkpoints, and complete tool history. MIT OR Apache-2.0. Six cited idioms carry strong human signals.

## Baseline: PromptForge already owns the rare language idea

PromptForge splits a Rust workspace into a Markdown parser, document executor, standalone Lua agent executor, shared Lua VM and coroutine protocol, model client, tool registry, store, gateway, Workshop server, and Tauri UI. Its document runtime makes prose a typed AST block and exposes explicit `reply`, `var`, store, `execute`, and `fanout`; its agent runtime exposes raw message roles, `models.chat`, event history, and blocking Workshop input. No shortlisted reference combines ordinary rendered Markdown with a sandboxed Lua controller this directly.

The split now blocks the proposed product. Document sections cannot invoke interactive or autonomous agent programs, while agent programs lack document control operations. Automatic compaction and arbitrary pinned messages do not exist. Built-in chat reconstructs only user and final assistant messages, so tool history is incomplete. Workshop's robust input lifecycle is not a generic executor contract, and production relaunch does not yet restore durable session files.

## Detailed findings, ranked by payoff

### Finding 1: Make every model-facing section one callable agent context

Tactus is the closest prior art. Its Lua procedures call stateless models, stateful tool-using agents, direct tools, human interactions, and child procedures as ordinary operations, while the execution context checkpoints side-effecting boundaries ([execution context](https://github.com/AnthusAI/Tactus/blob/08fc62ee2fcb6ccf58d0467a580551c7c7d6c121/tactus/core/execution_context.py#L228-L440), [handles](https://github.com/AnthusAI/Tactus/blob/08fc62ee2fcb6ccf58d0467a580551c7c7d6c121/tactus/primitives/handles.py#L90-L292), [tools](https://github.com/AnthusAI/Tactus/blob/08fc62ee2fcb6ccf58d0467a580551c7c7d6c121/tactus/primitives/tool.py#L77-L152), [human input](https://github.com/AnthusAI/Tactus/blob/08fc62ee2fcb6ccf58d0467a580551c7c7d6c121/tactus/primitives/human.py#L141-L213), [child procedures](https://github.com/AnthusAI/Tactus/blob/08fc62ee2fcb6ccf58d0467a580551c7c7d6c121/tactus/primitives/procedure_callable.py#L63-L222)). IronCrew independently launches a fresh Lua subflow from any runtime VM and transfers only bounded JSON ([subflow contract](https://github.com/skitsanos/ironcrew/blob/48cb8376cd9c587f85daf99266ac36f6562018c1/src/lua/subflow.rs#L14-L180)).

Leeway and agent-runtime confirm that this is not a niche Lua idea. Leeway wraps a fresh bounded model loop in each deterministic graph node ([node schema](https://github.com/hardness1020/Leeway/blob/7601e5efe1a341374380d8a11409c4d85597cc92/src/leeway/workflow/types.py#L90-L117), [execution](https://github.com/hardness1020/Leeway/blob/7601e5efe1a341374380d8a11409c4d85597cc92/src/leeway/workflow/engine.py#L171-L328)); agent-runtime makes an agent invocation a normal workflow step ([AgentStep](https://github.com/tsharp/agent-runtime/blob/07ebca8ec36d15eae2d264d4998fa6857f9e0b51/src/workflow/steps/agent.rs#L30-L78)).

PromptForge should replace its sibling-executor split with one section agent context. Markdown prose remains readable prompt data, while following Lua decides whether to create a system message, pin user context, perform one model round, run a complete tool loop, wait for user input, or launch a child episode. This retains PromptForge's strongest idea and adopts the field's convergent episode boundary. Confidence: high - four independent implementations converge on deterministic outer control around autonomous inner loops.

### Finding 2: Build compaction as a projection over durable history

Goose supplies the strongest implementation. It preserves original messages for the user and audit history while making compacted messages invisible to the agent, then appends an agent-only summary and continuation context ([context manager](https://github.com/aaif-goose/goose/blob/5e90925962f05acf8e255032de44d16c4a7768a2/crates/goose/src/context_mgmt/mod.rs#L70-L202)). Proactive compaction and typed context-error recovery preserve the current user request and active turn state ([compaction operation](https://github.com/aaif-goose/goose/blob/5e90925962f05acf8e255032de44d16c4a7768a2/crates/goose/src/agents/state_machine/ops_compaction.rs#L220-L313)).

Leeway corroborates the policy layering. It first clears stale tool-result bodies, then summarizes older messages without tools while retaining recent messages ([query loop](https://github.com/hardness1020/Leeway/blob/7601e5efe1a341374380d8a11409c4d85597cc92/src/leeway/engine/query.py#L53-L96), [compaction](https://github.com/hardness1020/Leeway/blob/7601e5efe1a341374380d8a11409c4d85597cc92/src/leeway/services/compact/__init__.py#L21-L129), [summary stage](https://github.com/hardness1020/Leeway/blob/7601e5efe1a341374380d8a11409c4d85597cc92/src/leeway/services/compact/__init__.py#L272-L317)). IronCrew supplies the last-resort invariant: remove complete old turn groups without splitting tool protocol pairs ([atomic eviction](https://github.com/skitsanos/ironcrew/blob/48cb8376cd9c587f85daf99266ac36f6562018c1/src/llm/provider.rs#L410-L504)).

PromptForge should preserve its event log as canonical truth and compact only the model projection. The default policy should preserve system and explicitly pinned messages, clear or summarize consumed tool bodies, summarize old turns, retain a recent verbatim tail, and finally fail or evict complete turns according to an author-selected policy. Confidence: high - Goose tests the mature form, while Leeway and IronCrew confirm the two lower layers.

### Finding 3: Validate and heal complete tool exchanges at one boundary

Goose repairs malformed arguments, denied calls, interruption, cancellation, and missing responses by generating correlated tool results before the next provider turn ([inference repair](https://github.com/aaif-goose/goose/blob/5e90925962f05acf8e255032de44d16c4a7768a2/crates/goose-agent/src/inference.rs#L190-L263), [tool dispatch repair](https://github.com/aaif-goose/goose/blob/5e90925962f05acf8e255032de44d16c4a7768a2/crates/goose/src/agents/state_machine/ops_toolcalling.rs#L930-L1019)). IronCrew validates one leading system message, complete assistant-call and tool-result pairing, and safe turn boundaries before dispatch and persistence ([history validator](https://github.com/skitsanos/ironcrew/blob/48cb8376cd9c587f85daf99266ac36f6562018c1/src/llm/provider.rs#L254-L504)). Tactus removes orphan tool results at the final provider boundary ([provider assembly](https://github.com/AnthusAI/Tactus/blob/08fc62ee2fcb6ccf58d0467a580551c7c7d6c121/tactus/dspy/module.py#L141-L258)). agent-runtime returns the complete call-result-final-text transcript from each episode ([agent loop](https://github.com/tsharp/agent-runtime/blob/07ebca8ec36d15eae2d264d4998fa6857f9e0b51/src/agent/mod.rs#L328-L507), [history tests](https://github.com/tsharp/agent-runtime/blob/07ebca8ec36d15eae2d264d4998fa6857f9e0b51/tests/chat_history_tests.rs#L132-L157)).

PromptForge should build provider messages through one context projector that validates or repairs every tool exchange immediately before dispatch. Durable events remain untouched. This eliminates the built-in chat agent's current loss of call and result history without forcing Lua authors to reconstruct provider protocol. Confidence: high - complete pairing is a runtime invariant in every relevant reference.

### Finding 4: Use one blocking-input protocol for Lua and model tools

Goose keys elicitation by session and tool-call identity, persists the human response before resolving the blocked future, rejects wrong-session and duplicate answers, and removes pending state on every completion path ([action manager](https://github.com/aaif-goose/goose/blob/5e90925962f05acf8e255032de44d16c4a7768a2/crates/goose/src/action_required_manager.rs#L49-L191), [persistence ordering](https://github.com/aaif-goose/goose/blob/5e90925962f05acf8e255032de44d16c4a7768a2/crates/goose/src/elicitation.rs#L24-L70)). IronCrew uses one run-scoped bridge for scripted Lua and model-visible human tools, and pauses task timeout accounting while a question is pending ([input bridge](https://github.com/skitsanos/ironcrew/blob/48cb8376cd9c587f85daf99266ac36f6562018c1/src/engine/input_bridge.rs#L1-L12), [human tool](https://github.com/skitsanos/ironcrew/blob/48cb8376cd9c587f85daf99266ac36f6562018c1/src/tools/ask_human.rs#L90-L199), [timeout accounting](https://github.com/skitsanos/ironcrew/blob/48cb8376cd9c587f85daf99266ac36f6562018c1/src/engine/task_runner.rs#L30-L69)).

Tactus persists replayable requests and races multiple attended or asynchronous channels under one interaction identity ([human primitives](https://github.com/AnthusAI/Tactus/blob/08fc62ee2fcb6ccf58d0467a580551c7c7d6c121/tactus/primitives/human.py#L141-L1008), [channel broker](https://github.com/AnthusAI/Tactus/blob/08fc62ee2fcb6ccf58d0467a580551c7c7d6c121/tactus/adapters/control_loop.py#L211-L435)). Leeway serializes questions from concurrent branches through one lock ([HITL broker](https://github.com/hardness1020/Leeway/blob/7601e5efe1a341374380d8a11409c4d85597cc92/src/leeway/workflow/hitl.py#L10-L41)).

PromptForge should retain Workshop's stronger tokenized wait and reconnect lifecycle, but move it into the common executor. Lua and optional model tools should call the same `user_input` primitive, while the launch host chooses blocking, immediate fallback, or failure. Confidence: high - host-owned durable elicitation is a clear field consensus.

### Finding 5: Bind effective context into episode identity

IronCrew fingerprints every non-secret input controlling a durable conversation: source, selected agent, effective system prompt, model, context limits, tool rounds, resolved tool graph, and provider behavior ([identity](https://github.com/skitsanos/ironcrew/blob/48cb8376cd9c587f85daf99266ac36f6562018c1/src/engine/conversation_identity.rs#L10-L84), [definition](https://github.com/skitsanos/ironcrew/blob/48cb8376cd9c587f85daf99266ac36f6562018c1/src/engine/conversation_definition.rs#L19-L84)). Tactus builds the exact provider payload, counts it with the selected model, hashes and authorizes it, then dispatches that same object without a second reconstruction ([payload build](https://github.com/AnthusAI/Tactus/blob/08fc62ee2fcb6ccf58d0467a580551c7c7d6c121/tactus/dspy/module.py#L105-L258), [attempt authority](https://github.com/AnthusAI/Tactus/blob/08fc62ee2fcb6ccf58d0467a580551c7c7d6c121/tactus/dspy/agent.py#L1897-L2074)). Goose rebuilds persistent and ephemeral system contributions in stable keyed order before inference ([prompt manager](https://github.com/aaif-goose/goose/blob/5e90925962f05acf8e255032de44d16c4a7768a2/crates/goose/src/agents/prompt_manager.rs#L19-L242)).

PromptForge should seal the effective system prompt, ordered tool schemas, model options, compaction policy, and projection rules at the first model call. Resume under changed values should create a new incarnation or fail explicitly. Confidence: high - the references make context identity testable and prevent old history from silently running under new authority.

### Finding 6: Return agent judgment through typed Lua-visible signals

Leeway derives the legal model decisions from outgoing graph edges, advertises them through a dedicated tool, rejects out-of-scope decisions, and evaluates the result deterministically ([legal signals](https://github.com/hardness1020/Leeway/blob/7601e5efe1a341374380d8a11409c4d85597cc92/src/leeway/workflow/types.py#L185-L194), [signal tool](https://github.com/hardness1020/Leeway/blob/7601e5efe1a341374380d8a11409c4d85597cc92/src/leeway/workflow/signal_tool.py#L10-L69), [evaluator](https://github.com/hardness1020/Leeway/blob/7601e5efe1a341374380d8a11409c4d85597cc92/src/leeway/workflow/evaluator.py#L20-L33)). Goose pairs typed recipe outputs with deterministic post-run checks and bounded retries ([recipe contract](https://github.com/aaif-goose/goose/blob/5e90925962f05acf8e255032de44d16c4a7768a2/crates/goose/src/recipe/mod.rs#L40-L128), [retry operation](https://github.com/aaif-goose/goose/blob/5e90925962f05acf8e255032de44d16c4a7768a2/crates/goose/src/agents/state_machine/ops_retry.rs#L229-L283)).

PromptForge should let `tool_loop()` return more than text: final text, a validated decision, structured data, usage, and completion status. Lua then owns the next edge without scraping control intent from prose. Confidence: high - typed outcome tools preserve model judgment while keeping orchestration deterministic.

### Finding 7: Persist reconstructible effects instead of executor stacks

Goose reloads durable session state before each operation, applies one operation effect, persists it, and can reconstruct the pipeline after every step ([machine](https://github.com/aaif-goose/goose/blob/5e90925962f05acf8e255032de44d16c4a7768a2/crates/goose-agent/src/machine.rs#L48-L171), [session effects](https://github.com/aaif-goose/goose/blob/5e90925962f05acf8e255032de44d16c4a7768a2/crates/goose/src/agents/state_machine/session.rs#L40-L149)). Tactus checkpoints each side-effecting Lua boundary and stores children or continuations as host-owned references ([checkpoint loop](https://github.com/AnthusAI/Tactus/blob/08fc62ee2fcb6ccf58d0467a580551c7c7d6c121/tactus/core/execution_context.py#L228-L564)). agent-runtime serializes a context checkpoint, restores it, and continues a later workflow ([context](https://github.com/tsharp/agent-runtime/blob/07ebca8ec36d15eae2d264d4998fa6857f9e0b51/src/context/mod.rs#L11-L95), [checkpoint tests](https://github.com/tsharp/agent-runtime/blob/07ebca8ec36d15eae2d264d4998fa6857f9e0b51/tests/checkpoint_tests.rs#L5-L170)).

PromptForge should add a versioned execution snapshot anchored to its append-only event offset. The snapshot should identify the section incarnation, replay position, active model projection, and outstanding input or child waits. Lua should replay from stable boundaries rather than serialize a coroutine stack. Confidence: medium - the effect-replay pattern is proven, but mapping existing block coroutines onto stable checkpoints needs design work.

### Finding 8: Split orchestration centers before adding context policy

Goose proves that inference, compaction, tools, recipes, and session effects can sit behind small operation contracts ([operation interface](https://github.com/aaif-goose/goose/blob/5e90925962f05acf8e255032de44d16c4a7768a2/crates/goose-agent/src/operation.rs#L72-L153)), but its product still carries parallel legacy and state-machine loops plus multi-thousand-line orchestration files. IronCrew enforces a size ratchet while carrying forty-eight explicit exceptions, including persistence modules above three thousand lines ([ratchet](https://github.com/skitsanos/ironcrew/blob/48cb8376cd9c587f85daf99266ac36f6562018c1/scripts/check_module_size.py#L1-L138), [exceptions](https://github.com/skitsanos/ironcrew/blob/48cb8376cd9c587f85daf99266ac36f6562018c1/scripts/module_size_policy.json#L21-L189)). Tactus concentrates runtime, agent, DSL, and IDE policy in modules between roughly 2,600 and 4,200 lines.

PromptForge's scheduler, Lua protocol, agent driver, and Workshop session registry already exceed one thousand lines. Context projection, compaction, episode identity, and interaction brokering should become separate modules before the executors merge. Confidence: high - every reference that left these concerns in central loops accumulated duplicate paths, unenforced declarations, or severe review surfaces.

## Provenance

Tactus was examined only at its pinned tip, so its cited idioms carry unknown provenance by operator instruction. IronCrew's subflow composition, durable identity, agent-as-tool, and system-tool identity mechanisms carry strong human signals; its history validator and part of timeout accounting carry explicit AI markers with no earlier form. All cited Leeway mechanisms were introduced in explicitly AI-marked commits and have no pre-AI form. Every Goose finding touches at least one explicitly AI-marked file, but available rewinds show that dual-visibility compaction, keyed system context, elicitation, delegation, and deterministic retry already existed and were retained or tightened. All cited agent-runtime idioms carry strong human signals.

This sample supports no broad claim about AI-authored code. The provenance tags price individual mechanisms only. In particular, Leeway's tip contains major ownership and lifecycle defects despite its coherent generated architecture, while Goose's rewinds show mature mechanisms surviving later AI-assisted modification.

## Where the subject already matches or beats the references

PromptForge's typed Markdown AST makes prose a clearer runtime object than Tactus task strings, Leeway YAML prompts, Goose recipe prose, or agent-runtime configuration. Its Lua request and answer protocol gives authors direct control over model rounds, tool calls, section execution, and fanout that none of the non-Lua references match. Workshop's existing input wait already matches or beats the field on single-use tokens, cancellation, reconnect replay, and byte-exact event recording. PromptForge also starts with the correct compaction foundation: durable events, ephemeral stream deltas, and section-local execution state are already distinct.

## Messes we should explicitly not copy

Tactus declares per-turn tools and turn ceilings that its concrete execution path does not enforce; persistence-shaped APIs are placeholders, local cancellation is cosmetic, and unattended escalation fails open. IronCrew's safe whole-turn eviction is not semantic compaction, and its size ratchet normalizes forty-eight exception files instead of repairing them. Leeway's compactor replaces a local list while the owning conversation retains stale history, callback wiring differs by entry path, and its model loop and compaction lack direct tests. Goose carries old and new agent loops in parallel, and some recipe sequencing remains prose policy rather than runtime behavior. agent-runtime advertises context pruning and OpenAI agents that are not fully wired, duplicates error families, and uses an unsafe downcast in subworkflow execution.

## Recommended execution order

1. Define one section-owned context contract with explicit system, user, assistant, tool, retention, and episode-result types. This establishes Findings 1, 3, and 5 before behavior moves.
2. Turn prose into substituted data for the following Lua block, expose explicit model-round and tool-loop calls, and route both existing executors through the section context. This completes Finding 1 without adding implicit terminal-prose behavior.
3. Add the provider-boundary context projector and complete tool-protocol healing from Finding 3.
4. Add exact request token accounting, system and pinned retention, recent-tail policy, semantic summary, and hard-fail compaction strategies from Finding 2.
5. Lift `user_input` into the generic wait protocol and expose the same broker to Lua and optional model tools, satisfying Finding 4.
6. Bind sealed context into incarnation identity and implement replayable execution snapshots from Findings 5 and 7.
7. Add typed episode signals and deterministic postconditions from Finding 6.
8. Keep the new projection, compaction, identity, and input broker modules outside the existing central files, then ratchet those files downward as required by Finding 8.

## Refactor notes

Findings 3 and 8 are primarily structural; Findings 1, 2, 4, 5, 6, and 7 change runtime behavior and public contracts. Existing parser, pipeline, agent, tool-loop, Workshop-session, and event-log tests are the invariant and move only after replacement coverage passes. Keep gateway provider conversion, store behavior, and Workshop presentation outside the first merger. Verify each step with focused crate tests and the workspace suite at component boundaries. Commit each verified slice separately. Stop and re-plan after two consecutive failures with the same signature on one step.

## Sources

- Tactus: https://github.com/AnthusAI/Tactus at `08fc62ee2fcb6ccf58d0467a580551c7c7d6c121`, MIT, analyzed 2026-09-06. Provenance intentionally not examined.
- IronCrew: https://github.com/skitsanos/ironcrew at `48cb8376cd9c587f85daf99266ac36f6562018c1`, MIT, analyzed 2026-09-06. Cited AI-originated files had no earlier form.
- Leeway: https://github.com/hardness1020/Leeway at `7601e5efe1a341374380d8a11409c4d85597cc92`, MIT, analyzed 2026-09-06. Cited files had no pre-AI form.
- Goose: https://github.com/aaif-goose/goose at `5e90925962f05acf8e255032de44d16c4a7768a2`, Apache-2.0, analyzed 2026-09-06. Rewinds: `5b93ee587feb4135146b27ad8683a9a9b6bd2feb`, `09c8d2be5aba1b6aa91794c21574cdd770c33ad9`, `6782d1f5062e4bc3a8371808bbd99ee05fa19b16`, `72da97204e123be70efb8d46f8217155bf83f404`, `4aa5de150a86a2c02d0fc45aec9819cad9cec5c2`, `838d99e0499433824016e93342563515230cb0f3`, and `fb47728f1b39a73bdc701b7e5890c39f11df2260`.
- agent-runtime: https://github.com/tsharp/agent-runtime at `07ebca8ec36d15eae2d264d4998fa6857f9e0b51`, MIT OR Apache-2.0, analyzed 2026-09-06.
- Field survey: sixteen candidates verified or classified on 2026-09-06; popularity values recorded that day.
- PromptForge subject profile: source tree profiled on 2026-09-06.

*2026-09-06 08:55 - GPT-5.6 Sol*
