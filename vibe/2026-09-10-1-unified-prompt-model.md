---
name: Unified Prompt Model
overview: "Implement a narrow `promptforge: 0` core: rename heading-based `execute` to `call`, add pending Markdown capture and lazy `prose`, remove `reply`, expose plain messages with optional builders, retain `models.infer`, add Rust-backed `models.loop` with an optional leading model handle, and implement only the minimum compactor surface with `compactors.fail` as the sole policy. Keep every other designed capability documented but deferred and inert."
todos:
  - id: define-unified-contracts
    content: Define only the active plain-message, message-builder, models.loop, and call-time model/tool selection contracts; retain every other contract as deferred description.
    status: pending
  - id: rename-section-call
    content: Rename heading-based `execute` to synchronous `call` as the first behavior change, preserving current execution semantics.
    status: pending
  - id: resolve-concurrency-primitive
    content: "Deferred: retain the designed `call_async`, task, `tasks.await_all`, and thin Lua fanout contracts without implementing or migrating them."
    status: deferred
  - id: replace-prose-semantics
    content: Replace positional prose inference with a pending Markdown buffer, thematic-break reset, and a read-only, lazy, memoized `prose` value for each following Lua fence.
    status: pending
  - id: add-message-list
    content: Ship optional pure-Lua `messages.new()` builders over the unchanged plain message-array contract.
    status: pending
  - id: add-context-projection
    content: Add only provider-neutral message projection and role and tool-call correlation validation required by active `models.loop`.
    status: pending
  - id: add-compactor-fail
    content: Add the minimum compactor surface `models.loop` requires (optional compactor parameter defaulting to `compactors.fail`, overflow invocation with reason, typed context exhaustion) and defer the replacement-handling framework.
    status: pending
  - id: add-models-loop
    content: Expose Rust-backed `models.loop(handle?, messages, compactor?)`, append complete assistant and tool history, return nil, and default omitted compaction policy to `compactors.fail`.
    status: pending
  - id: add-modules-execution
    content: "Deferred: retain inherited Lua, `require`, path-based `execute`, `execute_async`, `include`, `local_include`, and Markdown fragments as documented future contracts."
    status: deferred
  - id: generalize-user-input
    content: Lift blocking, fallback, and failing user input into a generic broker used by direct Lua `user_input()`, the model-visible input tool, and the Agent window.
    status: pending
  - id: restore-unified-sessions
    content: "Deferred: retain journaled operations, snapshots, incarnation-gated restore, and replay contracts."
    status: deferred
  - id: migrate-prompts-guides
    content: Migrate only prompts, fixtures, APIs, and guides directly affected by `execute` to `call`, `tool_call` to `tools.call`, `handle:infer` to `models.infer(handle, prompt)`, explicit lazy prose, `reply` removal, message builders, `models.infer`, and `models.loop`.
    status: pending
  - id: verify-complete-model
    content: Run focused parser, Lua, models.loop, projection, and migration verification for the active core only.
    status: pending
isProject: false
---

# Unified PromptForge Prompt Model

**Active implementation scope.** Rename heading-based `execute` to synchronous `call`; add pending Markdown capture with thematic-break reset and lazy `prose`; remove `reply`; retain plain message arrays and add optional pure-Lua builders; retain `models.infer`; expose Rust-backed `models.loop(handle?, messages, compactor?)`; implement the minimum compactor invocation surface with `compactors.fail` as the only policy; and generalize `user_input()` enough to keep the Agent window working.

Only focused tests and migrations required by those active changes belong to this plan.

**Deferred scope.** Every other contract remains documented but inert, with no implementation step, test requirement, migration requirement, or blocker.

<product-contract>

## Product Requirements

- Problem and users:
  - Prompt authors currently face two non-composable executors: implicit Markdown prose execution for finite pipelines and standalone Lua programs for interactive agents.
  - Prompt authors need one model that supports autonomous tool loops, interactive user turns, compaction, subagents, and deterministic section control without forcing routine prompts to manage raw message records.
  - Host integrators need the same editable Lua behavior in Workshop, CLI, automation, and headless execution without private Rust-only agent policy.
- Goals:
  - Treat every model-facing section as an agent context with one fresh Lua VM per section entry.
  - Treat Markdown accumulated since the nearest preceding heading, Lua fence, or thematic break as a read-only lazy template exposed as `prose` to the following Lua fence and evaluated only on its first runtime read.
  - Make every model operation explicit in Lua, with no final-prose or positional inference rule.
  - Provide one Rust-backed `models.loop(messages, compactor?)` over a provider-neutral message model; it appends complete assistant and tool history, invokes the selected compactor when needed, defaults omission to `compactors.fail`, and returns nil.
  - Implement only the minimum compactor surface `models.loop` requires: an optional compactor parameter defaulting to `compactors.fail`, invocation on precheck or provider overflow, and typed context exhaustion from `compactors.fail`.
  - Defer the replacement-handling compactor framework: custom compactor callbacks, budget records, replacement validation, measurable progress, bounded retry, in-place history replacement, shipped summarization, other built-in compactor strategies, pin helpers, compaction events, and compaction persistence.
  - Defer lexical Lua inheritance, confined modules, and complete child-prompt execution.
  - Defer child concurrency and task-aware store changes.
  - Defer event-view removal, durable replay, and cold restart.
  - Preserve Lua ownership of orchestration, message construction and reshaping, result selection, compaction policy, and interaction policy while Rust owns model-tool continuation, protocol-safe history append, and scheduling.
  - Retain Mentograph-style background research and Papergate-style ordered parallel evaluation as deferred acceptance scenarios for the future task runtime.
- Non-goals:
  - Do not fork or replace the gateway's model-specific chat-template responsibility.
  - Do not make Rust choose future summary wording or retention policy.
  - Active host policy chooses blocking, unavailable fallback, or failure for user input; Lua decides whether returned input enters messages.
  - Do not share mutable Lua state between sections.
  - Do not add implicit trailing-prose behavior until measured authoring evidence justifies it.
  - Do not redesign Workshop presentation, gateway administration, or speech-to-text.
  - Do not redesign store semantics beyond integration required by the active runtime.
  - Do not implement path-based `execute` or `execute_async` in this plan; reserve their target contract for later work.
  - Do not implement `require`, `include`, `local_include`, or partial Markdown fragments in this plan. The include APIs and fragment artifact do not currently exist, so the current plan has no removal, compatibility, testing, or user migration work for them; later plans will implement them.
- Success criteria:
  - One public executor runs finite pipelines, autonomous episodes, and persistent interactive agents from Markdown plus Lua.
  - Every model request originates from an explicit Lua call and carries a validated provider-neutral role sequence.
  - One Rust-backed `models.loop` appends protocol-complete history, processes every structured model tool call, returns nil, and behaves identically with zero or many model-visible tools.
  - The Agent window remains functional through the generic input broker and active `models.loop`.
  - Reconnect persistence, Workshop restore, concurrency, and composition contracts remain deferred.
  - Prompts, examples, guides, and tests touched by the active core use `promptforge: 0` and its explicit model.
- Constraints:
  - Keep `promptforge: 0`; never propose incrementing it while this rule remains active.
  - Preserve fresh VMs for fall-through, jump, call, and every child-task entry; use explicit task input, `var`, return values, and store as transfer channels.
  - Keep canonical events append-only and separate from mutable model-facing message arrays.
  - Keep the canonical event log host-owned and unavailable to prompt Lua; expose specific returned values or narrow APIs instead of a general event view.
  - Read model and tool selections at call time; the message array is the only continuity state and the author owns it. Direct Lua `tools.call` may access bound but unadvertised tools.
  - Keep all user-visible agent behavior editable at runtime through Lua or prompt files.
- Deferred open questions:
  - Define how a continuing conversation, mutable message projection, compactor policy, and pinned prefix receive stable identities that survive replay without serializing Lua table or closure identity.
  - Define operational semantics for frontmatter `input` and `output`.
  - Define H1 bootstrap ownership. `sys` remains immutable; model identity, finish reason, request IDs, projection hashes, and metrics remain host-observer data rather than later `sys` mutation or Lua models.loop results.

## Functional Specification

- Actors and workflows:
  - PromptForge accumulates Markdown in a pending prose buffer after each section heading or ordinary Lua fence. A Markdown thematic break recognized by the parser clears that buffer and is not included in `prose`.
  - On each ordinary Lua fence, PromptForge installs the current buffer as a fresh unresolved read-only `prose` value, clears the pending buffer, and runs the Lua coroutine.
  - The first runtime read snapshots section state, evaluates every `{{ }}` substitution once, memoizes the resulting string, and returns that same string on later reads.
  - Markdown before the last thematic break is inert commentary, and Markdown left after the section's final Lua fence is inert trailing commentary. Unconsumed Markdown at section end is discarded without error or model invocation.
  - A routine tool-free pipeline author calls `models.infer(prose)` and passes selected results explicitly through `var`, store, or return values rather than a rolling `reply` register.
  - A stateful or tool-capable author builds a plain message array directly or through optional pure-Lua `messages.new()` methods, calls `models.loop`, and may remove or reshape records before the next model operation.
  - An interactive author may call `models.loop` repeatedly over one retained message array and use direct `user_input()` or the model-visible input tool through one generic broker.
  - Persistence around that interactive message array remains deferred.
  - Deferred task contract: an interactive author may start isolated background child work, continue through `user_input`, and consume completed child results on a later turn without blocking the section VM.
  - An active host input policy blocks for a human, reports unavailable immediately, or fails; `user_input()` returns `text, available`, preserving the exact fallback sentence while distinguishing it from human speech. The model-visible input tool uses the same broker and correlated tool protocol.
  - A prompt author uses `call("## Heading", input?)` for an in-document section.
  - The deferred module contract uses sandboxed `require("./path.lua")` for one cached Lua module instance per VM.
  - The deferred complete-document contract uses `execute("./path.md", input?)` for a child PromptForge document.
  - Deferred task contract: `call_async(heading, input?)` eagerly creates and enqueues a section task, returns its handle without waiting, and lets the executor control when queued work becomes running.
  - Active `call` directly and synchronously invokes a section while preserving current execution behavior.
  - Deferred task and document contracts later define `call` as an await-and-unwrap wrapper and add symmetric `execute` and `execute_async`.
  - Deferred fanout contract: thin Lua creates one `call_async` task per ordered input and delegates scheduling, input-order aggregation, all-settled behavior, and atomic fail-fast cancellation to host-backed `tasks.await_all`.
- Inputs and outputs:
  - `models.loop(messages, compactor?)` accepts a plain JSON-representable message array or compatible `messages.new()` list and uses the section's current `models.*` selection and model-visible tool scope at call time. Omitted `compactor` means `compactors.fail`. With an explicit leading handle, `models.loop(handle, messages, compactor?)` runs on the handle's frozen binding at any time, independent of the section's `models.*` selection.
  - `models.loop` is implemented in Rust. It appends each assistant message, dispatches every structured tool-call batch, appends every correlated tool result, invokes the selected compactor when required, repeats until it appends terminal assistant text, and returns nil.
  - On successful return, the terminal assistant record is `messages[#messages]`; callers may read it, retain it, remove it with `messages[#messages] = nil`, or otherwise reshape the projection.
  - `messages.new()` returns a normal numerically indexed Lua table with optional chainable `system`, `user`, `assistant`, `tool`, and `append` methods; the host continues to validate the underlying plain records.
  - A tool-call assistant message may contain visible text plus multiple normalized calls; every tool result carries the matching call ID.
  - Host-observer statistics remain out-of-band from provider content and Lua models.loop results; they include attempt ID, projection hash, model, token usage, cached and reasoning tokens, finish reason, timing, and provider message ID when available.
  - Deferred task input is a deeply copied read-only JSON-representable value exposed to the child as `args`; strings remain valid, omitted input inherits the caller's current `args`, and no mutable Lua reference crosses a VM boundary.
  - Deferred `call_async` returns one opaque task with a stable read-only `id`, nonblocking idempotent `result() -> outcome|nil`, suspending `await() -> outcome`, and durable `cancel(reason?) -> boolean`.
  - Deferred `tasks.await_all(handles, options) -> outcomes` is implemented in Rust, accepts a dense ordered array, and supports all-settled or atomic fail-fast behavior.
  - An immutable outcome is `{ status = "succeeded", value = string|nil }`, `{ status = "failed", error = TaskError }`, or `{ status = "cancelled", error = TaskError }`; nil means the target completed without a scalar result. `TaskError` carries stable `family`, `code`, `message`, `retryable`, and originating `task_id` fields.
- States and validation:
  - Keep frame lifecycle, model or tool operation state, and child-task state as separate state machines rather than one progression.
  - Deferred child tasks move through `queued`, `running`, `waiting`, `closing`, and one terminal state: `succeeded`, `failed`, or `cancelled`; parent closure and child failure policy remain inert in the active core.
  - `models.loop` reads the section's current `models.*` selection and model-visible tool scope at each call; an explicit leading handle argument runs at any time on the handle's frozen binding. Provider projection is computed per dispatch for whichever model the call targets.
  - A section waiting on `user_input` retains its VM and message history until input, host failure, or cancellation resumes or terminates it.
  - Multiple leading system messages are legal in Lua arrays; the provider projector composes or maps them according to provider requirements without mutating the source array.
  - The context projector validates roles, content parts, unique tool-call IDs, complete call-result pairing, and provider-required alternation immediately before dispatch.
  - Streaming fragments form one assistant result; one provider request creates one assistant event even when it requests multiple tools.
  - The active compactor surface passes the overflow reason to the selected compactor; `compactors.fail` is the only shipped policy and always raises typed context exhaustion. The deferred compactor framework defines budget records, replacement arrays, and the sealed-prefix, atomic tool-exchange, and measurable token-budget progress invariants that replacements must preserve.
  - Deferred: Rust copies an accepted compactor replacement into the original history table.
  - Deferred compactor factories may later capture pinned prefixes or other retained records in Lua closure state.
  - Every prose-Lua pair owns a separate lazy value; entering the next pair discards the previous unresolved value, while Lua may preserve an already rendered string in a section global or `var`.
- Errors and recovery:
  - Unconsumed Markdown is never a parse error. Assigning to `prose`, recursively substituting `{{ prose }}`, or resolving an invalid placeholder raises at the first read site.
  - Any number of thematic breaks may clear pending prose; thematic breaks never terminate calls, prevent fall-through, or impose trailing-content restrictions.
  - Invalid message arrays, split tool exchanges, path escapes, incarnation drift, and context exhaustion produce typed errors at their owning boundaries; malformed compactor output, module, and child-prompt cycle errors belong to deferred contracts.
  - Recoverable tool failures, denial, cancellation, and interruption become correlated tool results when continuation is valid.
  - On precheck or provider overflow, `models.loop` invokes the selected compactor with the corresponding reason; `compactors.fail` always raises typed context exhaustion. Malformed replacement, repeated no-progress, and exhausted-policy handling belong to the deferred compactor framework.
  - Deferred summarizing compactors may call tool-free `models.infer` with or without an explicit handle, without recursion or changes to the section's model-visible tools.
  - The active input broker records waits and responses through existing host observation without adding the deferred replay contract.
  - Deferred persistence compares incarnation before replay, creates a fresh VM at section entry, and replays recorded host-call results without repeating effects.
  - Active synchronous `call` raises typed section errors. Deferred task contracts define creation, outcome, cancellation, and await failures.
- Security and privacy behavior:
  - Deferred composition contract: resolve required modules and executed prompts relative to the source file containing the call, only beneath declared roots; reject bare names, absolute paths, cycles, escapes, and limit breaches; freeze file contents for the run and preserve source paths in diagnostics.
  - Deferred composition contract: treat executed prompts and required modules under their existing trust envelopes; loading or child execution never promotes untrusted data to system authority silently. Active tool output and host context retain their existing trust envelopes.
  - Keep credentials, provider secrets, and wait tokens outside events, incarnation hashes, Lua values, model projections, snapshots, diagnostics, and logs.
  - Record fallback user-input results as host or tool events, never as real user messages; Lua alone decides whether to append returned text to a message list.
- Acceptance criteria:
  - Papergate-style finite pipelines use explicit `var`, store, return-value, or child-task handoffs and contain no `reply` compatibility path.
  - Dokuman-style deterministic pipelines launch compacting exploratory episodes and optional human gates.
  - Architect and Vibe Coder-style prompts remain interactive across multiple turns and persist operational truth outside conversational compaction.
  - The Agent window remains usable through explicit Markdown and Lua calls to Rust-backed `models.loop` plus the generic input broker, with complete tool history and no positional or implicit model invocation.
  - No Lua global or runtime API exposes the canonical event log.
  - Deferred acceptance scenario: Mentograph-style interaction launches background domain research without delaying the next question and safely consumes late results after one or more user waits.
  - Deferred acceptance scenario: Papergate-style evaluation runs child workers concurrently, preserves input order when aggregating, and applies explicit fail-fast `fanout` or protected all-settled `pfanout` without fanout-specific store semantics.

</product-contract>

<implementation-contract>

## Technical Design

- Architecture:
  - Collapse the public split in [`crates/promptforge/src/lib.rs`](C:/Users/Vinnie/cursor/promptforge/crates/promptforge/src/lib.rs) behind one run configuration and scheduler while retaining lower-level parser, Lua, model-client, tool, store, gateway, and observer crates.
  - Keep `SectionContext` as the owner of one fresh VM, plain Lua message arrays, current model and tool selection, counts, and lifecycle; move active context projection, the minimal compaction invocation path, and input brokering into focused modules and defer incarnation.
  - Keep the provider request as an internal Rust leaf and expose only Rust-backed `models.loop` to Lua, with an optional leading handle argument; ship optional message-list builders and `compactors.fail` now and defer the replacement-handling compactor framework, summarizing, and other built-in compactor factories.
  - Preserve the append-only `EventLog` as canonical truth and treat every Lua message array as an author-controlled model projection.
  - Deferred task architecture puts asynchronous child execution behind one task runtime with stable task and frame IDs; active `call` retains current synchronous execution semantics.
- Modules and interfaces:
  - Remove `loop_capable` from `Block::Prose` in [`crates/promptforge-parser/src/lib.rs`](C:/Users/Vinnie/cursor/promptforge/crates/promptforge-parser/src/lib.rs) and represent Markdown accumulation, thematic-break reset, and Lua-fence consumption without an unpaired-prose error.
  - Replace automatic prose advancement in [`crates/promptforge-core/src/execute/scheduler.rs`](C:/Users/Vinnie/cursor/promptforge/crates/promptforge-core/src/execute/scheduler.rs) with installation of the pending Markdown buffer as a fresh lazy prose template before each Lua coroutine starts; clear the buffer at thematic breaks, Lua fences, and section boundaries.
  - Generalize the existing agent-only chat request and Rust tool loop in [`crates/promptforge-lua/src/protocol.rs`](C:/Users/Vinnie/cursor/promptforge/crates/promptforge-lua/src/protocol.rs) and [`crates/promptforge-core/src/execute/tool_loop.rs`](C:/Users/Vinnie/cursor/promptforge/crates/promptforge-core/src/execute/tool_loop.rs) into section-visible `models.loop`, retaining provider-neutral messages, tool dispatch, cancellation, streaming, and host metrics.
  - Read model selection and tool scope at call time; `models.use`, `tools.add`, and `tools.add_local` remain available throughout the section and affect the next `models.loop` call.
  - Add request precheck, provider-overflow detection, and compactor callback invocation required by active `models.loop`; defer replacement validation, measurable progress checks, and atomic in-place history replacement to the deferred compactor framework.
  - Ship optional pure-Lua `messages.new()` message-list methods over a normal numeric table; `models.loop`, substitution, and protocol validation continue to consume the underlying plain records.
  - Move Workshop's wait registry behind a generic input-broker interface; expose direct `user_input()` returning `text, available`, adapt the model-visible input tool to the same broker, and preserve Agent-window behavior without adding cold restore.
  - Implement the Agent-window model picker as a deliberate minimal hack, to be revisited: `ui().selected_model` serves the MenuBus selection only for the Workshop Agent-window session and is nil in every other context (the `ui` provider is already a per-run host-injected closure, so this is wiring in `workshop-server`, not new machinery); in the Agent-window context, `models.get` resolves an undeclared alias as a raw gateway catalog model id, so `chat.md` calls `models.loop(models.get(ui().selected_model), messages)` and re-reads the selection each turn. The operator directed: "I want the smallest possible hack for ui().selected_model. Just make it work, with the absolute minimum amount of code, because we will have to revisit that anyway."
  - Deferred: add `lua inherit` as a contiguous initialization prefix replayed once per fresh VM.
  - Retain the deferred `require(path)` target contract: load a Lua module return value once per VM through PromptForge's frozen source-relative resolver rather than `package.path`, process working directory, native loaders, or unrestricted searchers. Do not implement it in the current plan.
  - Retain the deferred `execute(path, input?)` and `execute_async(path, input?)` target contracts for complete external `.md` PromptForge documents: resolve and freeze the source, invoke the same parser and executor contract as a root prompt, create isolated child VMs and prompt state, share only the run-scoped store and host policy, link child lifecycle events, and expose synchronous result or asynchronous Task semantics. Do not implement these interfaces in the current plan.
  - Deferred: add eager queued `call_async`, immutable task handles and outcomes, Rust-backed `tasks.await_all`, model-visible subagent adapters, continuation-capable waits, and journaled suspending operations.
- File and public API changes:
  - Parser and syntax: [`crates/promptforge-parser/src/build.rs`](C:/Users/Vinnie/cursor/promptforge/crates/promptforge-parser/src/build.rs), [`crates/promptforge-parser/src/fence.rs`](C:/Users/Vinnie/cursor/promptforge/crates/promptforge-parser/src/fence.rs), and parser tests lose positional prose semantics and gain pending Markdown capture with thematic-break reset; inherited initialization and complete-document execution remain deferred.
  - Lua and model substrate: [`crates/promptforge-lua/src/coro.rs`](C:/Users/Vinnie/cursor/promptforge/crates/promptforge-lua/src/coro.rs), its embedded Lua shims, VM setup, model bindings, protocol, dispatch, and errors expose the unified leaf calls and libraries.
  - Consolidate the Lua tool bindings into a `crates/promptforge-lua/src/tools/` module directory mirroring the existing `models/` layout: `mod.rs` for namespace installation (`always`, `add`, `add_local`, `call`, `calls`), `userdata.rs` for `LuaToolHandle` (moved out of `handles.rs`), `decode.rs` for alias-or-Tool argument polymorphism and `add_local` schema building, and `tests.rs`; move tool-installation logic out of `vm.rs`. Add `crates/promptforge-lua/src/messages/` in the same layout for the message namespace and its pure-Lua builders shim.
  - Executor: section context, scheduler, scope, errors, substitution, models.loop protocol harnesses, and public run options adopt one section-agent contract.
  - Active host integration covers the generic input broker and minimum Agent-window wiring; cold restore, incarnation checks, reconnect persistence, external Markdown-agent discovery, and broader Workshop migration remain deferred.
  - Rewrite the built-in Workshop chat agent from the standalone `chat.lua` program into an embedded `chat.md` Markdown prompt at `promptforge: 0` on the unified runtime: explicit retained message list, the `user_input()` broker, and `models.loop` with the leading handle for the UI-selected model. The chat agent becomes an ordinary prompt document; only external Markdown-agent discovery stays deferred.
  - Active public facade and guide changes cover `call`, prose, `reply` removal, messages, `models.infer`, `models.loop`, the minimal compactor surface and `compactors.fail`, and `user_input`.
- Data, persistence, failure, security, and privacy constraints, all deferred except active message validation, input waiting, and existing trust behavior:
  - Persist completed canonical events, content-free compaction records, sealed incarnation hashes, and execution snapshots tied to event offsets; snapshots carry block position, active projections, completed host-call results, and outstanding waits or children, never live coroutines or mutable userdata.
  - Snapshot a consistent task-graph cut at durable fence or host-operation boundaries with per-frame replay cursors, stable conversation IDs, operation-journal position, store version, pending waits, task outcomes, cancellation state, and live-child identities; reconstruct VMs by deterministic replay from frame entry because coroutine stacks, closures, globals, and module caches are not serializable.
  - Compaction records carry projection hashes, source spans, policy identity, reason, estimates, and counts, never replacement content, framing, injected files, schemas, mutable Lua objects, or secrets.
  - Hash prompt source, non-secret host policy, and the sealed identities of continuing conversations into the section incarnation; add frozen required-module bytes and executed child-prompt source only when their deferred composition contracts are implemented.
  - Keep tool-call batches and all answering results atomic during validation, compaction, truncation, cancellation repair, and replay.
  - Bound subagent recursion, compactor attempts, models.loop turns, context growth, outstanding waits, and restart replay; module and nested prompt recursion bounds belong to deferred composition contracts.
  - Preserve stable source locations and causal error chains across Lua yields, model attempts, compaction, and host waits; extend them to required modules and executed prompts when those deferred contracts are implemented.
  - Default to eight running child tasks across a run and depth eight. `call_async` eagerly creates and enqueues tasks; executor saturation leaves them queued, and a separate finite host-configured live-task bound covers queued, running, waiting, and closing tasks and rejects creation when exhausted.
  - Active `user_input` suspension preserves the section VM and follows host wait and cancellation policy; deferred background children later continue under their own inherited deadlines while a parent waits.
  - Derive stable opaque task IDs from run identity, incarnation, frame activation, and asynchronous-call operation identity. Host events link task, parent, frame, call operation, target, lifecycle transition, and optional tool call; Lua receives only handles and outcomes.
  - Deferred store changes make calls linearizable and journaled, detect causally unordered destructive mutations, and preserve atomic concurrent appends.
  - Restore under one exclusive run ownership epoch, verify operation kind and argument digests during replay, and reopen scheduling only after all active frames are reconstructed. Reattach uncertain external operations by stable operation ID when supported, retry only when non-execution or deduplication is proven, and otherwise fail the task with `indeterminate_operation`.

</implementation-contract>

<verification-contract>

## Testing Plan

- Unit:
  - Parser tests cover pending Markdown capture, reset at headings, Lua fences, and thematic breaks, leading and per-fence commentary exclusion, naturally inert trailing commentary, no unpaired-prose error, and `promptforge: 0` acceptance; inheritance and module-cycle tests remain deferred.
  - Lua protocol tests cover every role and content variant, visible text plus tool calls, multiple calls, correlated results, malformed arrays, `messages.new()` builders, read-only lazy `prose`, and direct input behavior; model metrics remain host-observer data.
  - Deferred event-sandbox tests cover eventual removal of `runtime.events()` while preserving host observation and replay.
  - Prose tests cover state mutation before first read, memoization after first read, no evaluation or error when never read, fresh evaluation for a second pair, assignment rejection, recursive `{{ prose }}` rejection, and `pcall` at the read site.
  - `models.loop` tests cover one terminal turn with no tools, repeated model-tool rounds with tools, automatic assistant and tool-result append, nil return, explicit terminal removal, local and bound tools, omitted-compactor default, `compactors.fail` invocation, typed context overflow, and explicit-handle `models.loop` calls on a frozen binding at any point in the section; custom replacement callback, in-place copy, malformed replacement, and no-progress detection tests remain deferred with the compactor framework.
  - Projection tests cover multiple leading system messages, same-role normalization, streaming coalescence, complete tool exchanges, abnormal-edge healing, and metadata stripping.
  - Deferred task-runtime tests cover background progress, stable handles, executor scheduling, `tasks.await_all`, fail-fast cancellation, parent closure, aggregation, store conflicts, journaling, replay, and restoration.
- Integration and end-to-end:
  - Migrate only parser fixtures, execution fixtures, shipped prompts, APIs, and guides directly affected by the active core and verify output parity.
  - Run one finite pipeline through explicit lazy prose, `models.infer`, `models.loop`, message builders, tool dispatch, `reply` removal, and synchronous `call`.
  - Run one Agent-window session through direct and model-visible user input, unavailable fallback, host failure, cancellation, and complete message history without requiring reconnect persistence.
  - Defer Mentograph background work, Papergate concurrency, task-runtime, event removal, reconnect, and cold-restart integration scenarios.
- Regression, security, and performance:
  - Preserve existing trust-envelope, near-duplicate tool, streaming, observer, and gateway wire tests touched by the core; cancellation, store race, and replay changes remain deferred.
  - Add malformed message, orphan tool record, duplicate call ID, and secret-exclusion tests required by active projection; defer compactor corruption, module, task, and replay adversarial tests.
  - Benchmark active message building, projection, models.loop, and the `compactors.fail` invocation path; defer inheritance, summarization, compactor-framework, task, module, and restore benchmarks.
- Exit criteria:
  - Focused tests pass for active parser, Lua, core, model-client, projection, input-broker, Agent-window, and migration changes.
  - Formatting, Clippy, affected crate tests, doctests, and architecture checks pass for the active slice; deferred persistence and broad platform matrices do not block it.
  - Touched production paths retain no implicit prose inference, `reply`, agent-only model API, incomplete tool projection, heading-based `execute`, bare `tool_call`, colon handle methods, or `promptforge: 1` examples.

</verification-contract>

<decision-record>

## Decision Record

- Decisions:
  - Use `promptforge: 0` throughout development. The operator's rule is: "as long as we are at 0 you will never suggest to increment it."
  - Keep one fresh VM per section entry and reuse it for every turn inside that section; explicit state channels preserve identical behavior across fall-through, jump, call, and child-task entry.
  - Name contained section invocation `call(heading, input?)`; it creates a fresh child frame, waits, returns the child's result, and resumes its caller, while `jump` transfers control without returning. The operator selected `call` after asking: "should we change execute() to call() in promptforge?"
  - Make the heading-based `execute` to `call` rename the first child-composition behavior change after the public contracts freeze; preserve its current synchronous behavior during the rename and verify no heading-form `execute` remains before introducing path-based `execute`. The operator decided: "renaming execute to call should be the first step (or close to it)."
  - Remove the special `reply` register and all automatic result handoff across fall-through and `jump`; authors use `var` or store for explicit transfer. The operator decided: "in the case of fallthrough or jump(section) we can just remove the `reply` feature. The author can use the store or a var to pass data".
  - Accumulate pending Markdown after each section heading or ordinary Lua fence, clear it at each thematic break, bind the remaining buffer as lazy `prose` for the next Lua fence, and discard any trailing buffer at section end. Thematic breaks affect only prose capture and never terminate calls or alter fall-through. This supersedes the prior deferred thematic control-flow design. The operator observed: "every prose can have commentary. the section can have commentary. and we can have trailing commentary."
  - Make prose a read-only, lazy, memoized variable for the following Lua block; resolve substitutions on first runtime read, not textual mention or block entry. The operator chose: "substitutions should not be evaluated until the first mention in `prose`" and clarified that every later prose block evaluates independently.
  - Expose one Rust-backed `models.loop(messages, compactor?)` model operation to Lua and keep the one-request provider turn internal to Rust. If no tools are visible, the loop performs one model call; if the model emits structured tool calls, Rust dispatches them, appends correlated results, and continues until terminal assistant text. Omitted compactor defaults to `compactors.fail`. The operator first asked "should we just have model_loop?" and then settled the name as `models.loop` after asking "consider: models.loop instead of model_loop?", keeping every model operation namespaced under `models.*` alongside `models.infer` and adding no new top-level global.
  - Keep exactly two tool execution paths: model-directed structured calls processed by Rust `models.loop`, and deterministic prompt-author calls through Lua `tools.call(alias_or_tool, arguments)`. Do not expose internal model-call batch dispatch or result-append machinery to Lua. The bare `tool_call` global is renamed to `tools.call` so every tool operation lives under the `tools.*` namespace alongside `tools.always`, `tools.add`, and `tools.add_local`, mirroring the `models.*` namespacing of model operations; the rename rides the same migration slice as `execute` to `call`.
  - Ship optional pure-Lua `messages.new()` methods over normal numeric message arrays to reduce role-string boilerplate while preserving raw-array compatibility and host validation.
  - Invoke the selected compactor on precheck or provider overflow and ship only `compactors.fail`, which always raises typed context exhaustion; defer custom replacement callbacks, invariant and measurable-progress validation, and in-place copy back into the caller's message table to the deferred compactor framework. The operator decided: implement only `compactors.fail` and the minimum compactor surface needed to make `models.loop` invocable with it.
  - Ship `compactors.fail` as the only built-in policy in this plan; defer `compactors.summarize`, pinning helpers, other built-in strategies, compaction persistence, and rich compaction events.
  - Select models only through `models.default`, `models.use`, or explicit handles, and select model-visible tools only through `tools.always`, `tools.add`, or `tools.add_local`; model calls accept no duplicate model or tool options. `models.get(alias)` returns a handle for a declared binding without changing the section's current selection; `models.use(alias)` selects and returns a handle.
  - Do not seal model or tool selections. `models.loop` reads the section's current `models.*` selection and tool scope at call time; an optional leading handle argument runs on the handle's frozen binding at any time. The message array is the only continuity state and the author owns it; providers never enforce selection stability because model identity and tool schemas are per-request. This lifts the current once-per-section restriction on `models.use`. The operator decided: "handle:loop can be used any time, and it does not seal the model or any of that shite."
  - Use namespace-only invocation with handles as pure inspectable values: `models.infer(handle?, prompt)`, `models.loop(handle?, messages, compactor?)`, and `tools.call(alias_or_tool, arguments)` take an optional leading handle or Tool object, and no userdata exposes colon methods. This removes the existing `handle:infer` method in the same migration slice. Chainable `messages.new()` builders remain the deliberate pure-Lua convenience exception. The operator decided: "Simpler implementation. Simpler mental model for authors. This is best for AI because only one way to do things."
  - Make `models.loop` append every assistant message and correlated tool result, including terminal assistant text as the final record, and return nil; callers that do not want the terminal record may remove it explicitly. The operator decided that complete history should be automatic and confirmed: "tool_loop(...) -> nil".
  - Return `text, available` from `user_input()` so unavailable mode can return the exact fallback sentence without allowing identical human text to spoof provenance.
  - Keep generic `user_input`, its model-visible tool adapter, and minimum Agent-window integration in active scope because the Agent window cannot function without an input wait. The operator decided: "user_input is required or else the Agent window stops working."
  - Allow `lua inherit` at every heading level but only as a contiguous initialization prefix before ordinary executable prose or Lua; applicable blocks execute once per fresh VM in deterministic root-to-leaf source order. The operator preferred `inherit` because the source is replayed rather than mutable state being shared.
  - Use a host-installed sandboxed `require("./path.lua")` for Lua modules; resolve from the source file containing the call, preserve that anchor when an inherited block is replayed, freeze bytes, cache one returned module value per VM, and reject bare names, absolute paths, process-working-directory lookup, unrestricted package searchers, and confined-root escapes. The operator accepted this contract after asking where `"helpers"` comes from and whether authors must implement `require`.
  - Use `execute("./path.md", input?)` only for complete external PromptForge documents and run them through the formal parser and executor as isolated child prompt invocations; use `call("## Heading", input?)` for in-document sections and `require("./path.lua")` for cached Lua module values. Partial Markdown composition through future `include` and `local_include` remains a distinct deferred contract.
  - Defer implementation of path-based `execute` and `execute_async` while preserving their full target contract in this plan and the language contract. The operator decided: "defer path-based execute. describe it in the plan but do not implement it".
  - Keep future `require`, `include`, `local_include`, and partial Markdown composition as distinct contracts: `require()` returns a cached Lua module value, while `include()` does not. None exists in the current implementation scope.
  - Defer implementation of `require`, `include`, `local_include`, and Markdown fragments while preserving their target descriptions. They require no current removal, compatibility, testing, or migration work because they do not yet exist, but later plans will implement them. The operator decided: "Lets defer require(). Keep the description but dont implement it as a step", clarified "There is no include path currently", and then settled: "they will eventually be implemented just not yet".
  - Keep canonical events private to the host and remove `runtime.events()` from Lua. The operator decided: "remove events".
  - Reserve the `markdown` namespace for future Lua access to the Markdown parser as a deferred contract, following the same namespace convention as `models` and `tools`; `list_from_section` remains a control-flow global because it resolves a heading over the document's visible set rather than parsing text. The operator anticipates: "eventually I am going to want to give Lua access to the markdown parser."
  - Keep `sys` immutable from section entry onward; model identity, finish reason, request IDs, projection hashes, and metrics remain host-observer data and do not appear in Lua models.loop results.
  - Use `call_async(heading, input?)` as the asynchronous section operation and keep `call(heading, input?)` as its synchronous await-and-unwrap wrapper. `call_async` eagerly creates and enqueues work but the executor controls when queued tasks become running. The symmetric path-based `execute` and `execute_async` taxonomy remains deferred.
  - Expose stable opaque task IDs, immutable terminal outcomes, nonblocking `result()`, suspending `await()`, durable `cancel()`, and Rust-backed `tasks.await_all()`; omit `ready()` because a non-nil result is the same terminal observation and defer `tasks.await_any` until a concrete race use case exists.
  - Keep ordered fanout only as thin shipped Lua over `call_async` and `tasks.await_all`; Rust owns scheduling, input-order aggregation, all-settled waiting, and atomic fail-fast cancellation. Removal of the core `fanout` global, `item`, `sys.index`, fanout result records, and fanout-specific store behavior is deferred until that task runtime lands; the existing core fanout remains installed and functional in the active core. The operator decided: defer fanout removal.
  - Use structured task lifetime without detached children: user-input suspension keeps children live, but parent completion closes spawning, cancels and drains nonterminal descendants, and publishes its outcome only after they become terminal. Child failure remains policy-neutral until Lua observes and propagates it.
  - Route model-visible subagent tools through the same task runtime as fixed-target spawn, await, and unwrap adapters; models receive serializable correlated tool results rather than task handles.
  - Treat every section as an agent context; pipeline, autonomy, and interactivity describe how Lua drives it, not separate section types.
  - Allow multiple leading system messages with provider-specific final projection; structural validation of roles and tool-call correlation happens per dispatch.
- Rejected alternatives:
  - Reject a shared VM across fall-through because it creates path-dependent hidden globals, uncloneable state, and inconsistent section inputs; revisit only if a serializable isolation mechanism preserves identical entry semantics.
  - Reject document-level pipeline and agent modes because one prompt can combine deterministic orchestration, autonomous exploration, and interaction; revisit only if measured implementations cannot preserve one runtime contract.
  - Reject implicit last-prose tool loops because they hide model execution; revisit only if authoring evidence justifies narrowly defined sugar.
  - Reject host-backed conversation ownership because the operator selected plain Lua message arrays and compactor closure state; revisit if durable replay proves impossible without a registered conversation object.
  - Reject retention fields in provider message arrays because retention controls future projection, not current model input; revisit if a provider-neutral projection metadata layer requires explicit record annotations.
  - Reject per-call model and tool options because they duplicate `models.*` and `tools.*` state and create competing sources of truth; revisit if a concrete workflow requires deliberate mid-section capability changes.
  - Reject sealing model and tool selections at first use because providers never enforce it (model identity and tool schemas are per-request), the author can already rewrite the message array between calls, and the guardrail only added machinery; revisit if a concrete workflow shows mid-section selection changes are a real authoring hazard.
  - Reject colon methods on handles (`handle:infer`, `handle:loop`, `handle:call`) despite PIL prescribing methods for host userdata and most ecosystems following that convention, because the Lua authors themselves call the colon confusing for newcomers, the dot/colon mixup is the most common scripting error, methods are not first-class values, and one namespace form gives authors and AI exactly one way to invoke anything; revisit only if authors consistently find the leading-handle argument unclear.
  - Reject Markdown headings, hidden comments, code-like prose fences, and custom paragraph directives as message-type syntax because Lua consumes rendered prose explicitly; revisit only after usability evidence shows Lua role construction is the dominant authoring burden.
  - Reject a rolling `reply` register because it conflates the latest assistant response, selected section output, and implicit inter-section transfer; revisit only if explicit handoffs prove materially unusable and one unambiguous meaning can be specified.
  - Reject `execute(heading, input?)` for intra-document sections because every section executes and the name does not communicate call-and-return behavior; revisit the naming only if user testing cannot distinguish `call` from external `execute`.
  - Reject off-walk, reader-only, and call-termination meanings for thematic breaks because one deterministic pending-prose reset explains commentary at section start, before every Lua fence, and after the final Lua fence.
  - Reject `lua shared` because it implies shared mutable state; revisit the spelling only if authors consistently misinterpret `lua inherit`.
  - Reject standard `package.path`, process-working-directory lookup, and unrestricted Lua loaders because module identity must remain source-relative, confined, frozen, and replayable; revisit only for an explicit signed package-root manifest.
  - Reject exposing a read-only canonical event-log view to Lua because it couples prompts to host event schemas, expands visibility and privacy rules, permits compaction bypass, and duplicates narrower return and metrics APIs; revisit only for a concrete policy that cannot use narrow counters, results, metrics, or store state.
  - Reject a core blocking `fanout` because it duplicates child creation and cannot express Mentograph's background work; thin Lua fanout delegates scheduling, aggregation, and failure policy to generic Rust task-container machinery.
  - Reject asynchronous options on `call` and deferred `execute` because option-dependent return types create competing paths; use the symmetric `call_async` and deferred `execute_async` names.
  - Reject a generic overloaded `spawn(target, input?)` surface because section and document target kinds deserve the same explicit distinction as `call` and `execute`; revisit typed targets only if a third target category appears.
  - Reject detached tasks because they make parent completion, cancellation, quotas, and cold restore ambiguous; revisit only for a durable daemon workload with an explicit owner outside the parent prompt.
  - Reject automatic parent failure when a child fails because best-effort Mentograph research must not terminate the interview; explicit Lua aggregation owns propagation policy.
- Assumptions, risks, and notes:
  - Generalizing the existing Rust loop into `models.loop` avoids a second model-issued tool-dispatch path; parity tests must still preserve current protocol, cancellation, streaming, and observer behavior.
  - Continuing one message array across different models through explicit-handle `models.loop` calls is permitted; providers cannot detect mixed-model histories because model identity is not part of the wire format. The real costs are loss of provider-specific reasoning artifacts (encrypted reasoning items, signed thinking blocks) during projection and possible quality drift, both author-visible rather than protocol violations.
  - Exact token counting may depend on provider or gateway tokenizer support; measured usage plus conservative estimates must remain safe when exact counting is unavailable.
  - Deferred nested formal prompt execution must preserve explicit child isolation while sharing only documented run services such as store, cancellation, deadlines, confinement, and host policy.
  - `lua inherit` blocks that perform host effects repeat those effects in every applicable fresh VM; guidance and tests should favor declarations, constants, and function construction.
  - Plain message arrays remain the protocol contract; optional `messages.new()` methods reduce boilerplate without creating host-owned conversation state, while deferred compactor factories will preserve immutable prefixes later.
  - Existing prompts require scoped migration because prose no longer infers automatically; thematic breaks now reset pending prose, and trailing Markdown becomes inert without becoming invalid.
  - The current executor accepts `promptforge: 1`; changing the gate, shipped prompts, and fixtures to `promptforge: 0` must be one controlled migration or a temporary dual-read transition, never a suggestion to increment the target language.
  - Pending Markdown capture, thematic-break reset, scheduler demotion of automatic prose, lazy `prose`, and `reply` removal are one coupled active behavior slice.
  - Current Workshop chat reconstructs messages through `runtime.events()`; migrate chat to the unified Markdown runtime before removing the event view. That rebuild-per-turn also provides today's restart and turn-cancel resume; after migration, message history lives in a Lua global and restart loses it until the deferred persistence work lands - an accepted interim regression.
  - The current sandbox removes `require`, current `execute` addresses headings, and current compaction, operation journaling, and incarnation snapshots are greenfield subsystems; source-relative module loading, external prompt execution, include APIs, and fragment artifacts remain documented deferred subsystems.
  - Future partial Markdown composition needs explicit parsing, lifecycle, scope, persistence, and return semantics before implementation so it does not drift into an implicit second prompt artifact.
  - Rust-backed `models.loop` still needs focused internals for projection budgeting, normalized call-ID dispatch, typed recoverable errors, and stable conversation and policy identity; these internals are not Lua APIs. Atomic compaction commit belongs to the deferred compactor framework.
  - Generalizing `args` from string to a read-only JSON-representable task input affects `call`, `execute`, substitution, frontmatter validation, and migration, but avoids a second child-input channel.
  - Exact cold restoration requires every control-flow-relevant host observation to be journaled and replay-verified, including nil task-result probes, completion selection, cancellation races, store reads, clocks, and host snapshots.
  - PromptForge can guarantee stable local task identity, replay-safe local store effects, and fail-closed indeterminate outcomes; remote cancellation and exactly-once external effects require provider or tool cooperation.

</decision-record>

<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build` (default member is `gateway`; the desktop app is explicit: `cargo build -p workshop`). UI bundles build through crate build scripts after `npm ci --prefix crates/workshop-server/ui` and `npm ci --prefix crates/gateway-config-ui/ui`.
- Focused test command pattern: `cargo test -p <crate> <name-filter>` (e.g. `cargo test -p gateway-stt --test it architecture`); nextest equivalent `cargo nextest run -p <crate> <filter>`.
- Component test command pattern: `cargo nextest run -p <crate>` (or `cargo test -p <crate>`); gateway process-ownership races run as named `cargo test --locked -p gateway --no-default-features --features test-fixtures --test it <name>` invocations.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`, then doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`; Workshop crates run separately on Windows: `cargo nextest run --locked -p workshop -p workshop-server`.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings` (Workshop: `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`).
- Formatter check command: `cargo fmt --all --check`.
- Test placement and naming conventions: unit tests live in `src` modules behind `#[cfg(test)]`; integration tests live in `tests/` as a `main.rs` harness pulling in per-area module files (gateway uses `tests/it/`, promptforge-core uses `tests/suite/` with `execution.rs`, `fanout.rs`, etc.); UI tests run with `npm test` inside `crates/workshop-server/ui` and `crates/gateway-config-ui/ui`. Nextest profiles in `.config/nextest.toml`; tensor/FFI-heavy crates (promptforge-tool-picker, gateway-stt, gateway-stt-backend-whisper) are pinned to a `heavy` test group.
- Directory map: `crates/` holds all Rust workspace members via the `crates/*` glob, named by product prefix (`gateway-*`, `promptforge-*`, `shared-*`, `workshop`, `workshop-server`, `product-integration-tests`); `crates/shared-ui` is a shared TypeScript+CSS package excluded from the Cargo workspace; `tools/` holds Node/PowerShell helper scripts (sidecar staging, MSRV validation); `guide/` holds the user guide; `prompts/` holds prompt assets; `vibe/` holds the architecture doc and working notes; `images/`, `local/` hold assets and local config; `.github/workflows/` holds CI; `.config/nextest.toml` configures nextest; `.cargo/config.toml` holds cargo config.
- Component boundaries (per `vibe/archdoc.md` and root `AGENTS.md`): executor (`promptforge-core`) parses and runs pipelines and Lua agent programs and depends on parser, Lua, model-client, store, tools, gateway, and shared substrate; `gateway` crates own model routing, provider access, and local inference; `workshop`/`workshop-server` host the desktop shell; `promptforge-store` is the run-scoped virtual filesystem; `promptforge-lua` is the Lua VM boundary; `shared-*` crates are the cross-product substrate. Dependency rules: PromptForge crates never depend on Gateway or Workshop crates; Gateway crates never depend on PromptForge or Workshop crates; Workshop crates never depend on Gateway crates.
- Conventions summary: Rust edition 2024, MSRV 1.89 pinned via `rust-toolchain.toml`; workspace lints forbid `unsafe_code` (except explicitly owned FFI boundaries with documented invariants) and deny clippy `all`, `unwrap_used`, and `expect_used`; comments explain non-obvious constraints and cite upstream issue URLs for workarounds; behavior changes ship with behavior tests in the same change; Cargo features gate real constraints (toolchain, native builds), never product shape; long-running work reports through `shared-progress`; library and serve paths return errors instead of exiting.

</project-survey>

<execution-plan>

## Execution Instructions

<step-1>

### Step 1: message record validation in the Lua protocol [completed]

- Component: messages

Extend message validation and typed request data in `crates/promptforge-lua/src/protocol.rs`: every role and content-part variant, visible text plus multiple normalized tool calls, correlated tool results carrying matching call IDs, and typed errors for malformed arrays. Unit tests live in the protocol module. Placement: first, because plain message records are the contract every later component consumes.

</step-1>

<step-2>

### Step 2: `messages.new()` builders module [completed]

- Component: messages

Add `crates/promptforge-lua/src/messages/` mirroring the `models/` layout: `mod.rs` for namespace installation, the embedded pure-Lua builders shim, and `tests.rs`. `messages.new()` returns a normal numerically indexed table with optional chainable `system`, `user`, `assistant`, `tool`, and `append` methods; host validation keeps consuming the underlying plain records. Tests cover builder output and raw-array compatibility. Placement: with Step 1 because both ship the message substrate; sequenced after it because builders are validated against the protocol records.

</step-2>

<step-3>

### Step 3: rename heading-based `execute` to `call`

- Component: section-call

Rename `execute(heading, input?)` to synchronous `call(heading, input?)` across the Lua bindings in `crates/promptforge-lua`, core execution and diagnostics in `crates/promptforge-core`, and affected tests, preserving current synchronous behavior and reserving path-based execution. Placement: the first behavior change per the decision record, before any section-invocation migration.

</step-3>

<step-4>

### Step 4: namespace-only tool and model invocation

- Component: section-call

Rename the bare `tool_call` global to `tools.call(alias_or_tool, arguments)` and remove colon handle methods so `models.infer(handle?, prompt)` and `tools.call` take an optional leading handle or Tool object. Consolidate the Lua tool bindings into `crates/promptforge-lua/src/tools/`: `mod.rs` for namespace installation (`always`, `add`, `add_local`, `call`, `calls`), `userdata.rs` for `LuaToolHandle` moved out of `handles.rs`, `decode.rs` for alias-or-Tool argument polymorphism and `add_local` schema building, and `tests.rs`; move tool installation out of `vm.rs`. Tests cover the renamed namespace and handle invocation forms. Placement: rides the same migration slice as the `execute` to `call` rename per the decision record.

</step-4>

<step-5>

### Step 5: parser pending Markdown capture

- Component: prose

In `crates/promptforge-parser/src/build.rs` and `crates/promptforge-parser/src/fence.rs`, remove `loop_capable` from `Block::Prose`, accumulate pending Markdown after each section heading or ordinary Lua fence, reset the buffer at thematic breaks without including the break in `prose`, and drop the unpaired-prose error. Parser tests cover capture, reset at headings, Lua fences, and thematic breaks, leading and per-fence commentary exclusion, inert trailing commentary, and `promptforge: 0` acceptance.

</step-5>

<step-6>

### Step 6: lazy `prose` and `reply` removal

- Component: prose

In `crates/promptforge-core/src/execute/scheduler.rs`, substitution, and VM setup in `crates/promptforge-lua`, replace automatic prose advancement with installing the pending buffer as a fresh read-only lazy `prose` template before each Lua coroutine starts; evaluate every `{{ }}` substitution once on first runtime read, memoize the result, and discard unconsumed buffers at section end. Remove the `reply` register and all automatic result handoff across fall-through and `jump`; authors use `var`, store, or return values. Prose tests cover state mutation before first read, memoization after first read, no evaluation or error when never read, fresh evaluation for a second pair, assignment rejection, recursive `{{ prose }}` rejection, and `pcall` at the read site. Placement: one coupled behavior slice with Step 5, split only for parser versus executor test ownership.

</step-6>

<step-7>

### Step 7: provider-neutral projection and per-dispatch validation

- Component: projection

Add context projection across `crates/promptforge-core` execution and the `crates/promptforge-lua` protocol: validate roles, content parts, unique tool-call IDs, complete atomic call-result pairing, and provider-required alternation immediately before dispatch; compose multiple leading system messages per provider without mutating the source array; coalesce streaming fragments into one assistant result; strip metadata. Projection tests cover multiple leading system messages, same-role normalization, streaming coalescence, complete tool exchanges, abnormal-edge healing, and metadata stripping, plus malformed message, orphan tool record, duplicate call ID, and secret-exclusion regression tests. Placement: after messages, before public `models.loop`.

</step-7>

<step-8>

### Step 8: minimum compactor surface with `compactors.fail`

- Component: compactor

Add request precheck, provider-overflow detection, and compactor callback invocation in the core execution path: an optional compactor parameter defaulting to `compactors.fail`, invocation with the overflow reason on precheck or provider overflow, and typed context exhaustion from `compactors.fail`. Defer budget records, custom replacement callbacks, replacement validation, measurable progress, bounded retry, and in-place history replacement. Tests cover the omitted-compactor default, `compactors.fail` invocation with reason, and typed context exhaustion. Placement: after projection, before `models.loop` overflow handling.

</step-8>

<step-9>

### Step 9: Rust-backed `models.loop`

- Component: models-loop

Generalize the agent-only chat request and Rust tool loop in `crates/promptforge-lua/src/protocol.rs` and `crates/promptforge-core/src/execute/tool_loop.rs` into section-visible `models.loop(handle?, messages, compactor?)`: read the section's current `models.*` selection and model-visible tool scope at each call, run an explicit leading handle on its frozen binding at any time, append every assistant message and correlated tool result including terminal assistant text as the final record, invoke the selected compactor on overflow, preserve streaming, cancellation, and host metrics, and return nil. Tests cover one terminal turn with no tools, repeated model-tool rounds, automatic assistant and tool-result append, nil return, explicit terminal removal, local and bound tools, omitted-compactor default, `compactors.fail` invocation, typed context overflow, and explicit-handle calls on a frozen binding. Placement: after projection and the compactor surface, before input integration.

</step-9>

<step-10>

### Step 10: generic input broker

- Component: input-broker

Generalize user input across `crates/promptforge-core` and `crates/promptforge-lua` into one broker with blocking, unavailable-fallback, and failure host policies: direct `user_input()` returns `text, available` so the exact fallback sentence cannot be spoofed by identical human text, and the model-visible input tool adapts the same broker through the correlated tool protocol. Record waits and responses through existing host observation without adding the deferred replay contract; a section waiting on input retains its VM and message history. Tests cover direct input, the model-visible input tool, unavailable fallback, host failure, and cancellation. Placement: after `models.loop`, which the model-visible input adapter runs inside.

</step-10>

<step-11>

### Step 11: Agent window on the unified runtime

- Component: input-broker

Move Workshop's wait registry behind the generic input-broker interface. Rewrite the built-in chat agent from the standalone `chat.lua` program into an embedded `chat.md` Markdown prompt at `promptforge: 0` on the unified runtime: an explicit retained message list, the `user_input()` broker, and `models.loop(models.get(ui().selected_model), messages)` re-reading the selection each turn. Implement `ui().selected_model` as the deliberate minimal hack: MenuBus selection wiring in `workshop-server` only for the Agent-window session, nil in every other context, with `models.get` resolving an undeclared alias as a raw gateway catalog model id in that context. Integration tests run one Agent-window session through direct and model-visible user input, unavailable fallback, host failure, cancellation, and complete message history without reconnect persistence. Placement: after the broker and `models.loop`, which it consumes.

</step-11>

<step-12>

### Step 12: migrate affected prompts, fixtures, and guides

- Component: migration

Migrate only fixtures, shipped prompts, public APIs, README material, and language and agent guides touched by the active core to `promptforge: 0`: `call`, `tools.call`, namespace-only handle invocation, explicit lazy prose, `reply` removal, plain message arrays or builders, `models.infer`, `models.loop`, `compactors.fail`, call-time model and tool selection, and generic input. Verify output parity on migrated fixtures and run one finite pipeline end to end through explicit lazy prose, `models.infer`, `models.loop`, message builders, tool dispatch, `reply` removal, and synchronous `call`. Finish with the project survey checks for the active slice: `cargo fmt --all --check`, Clippy with `-D warnings`, affected crate tests, doctests, architecture checks, and benchmarks for message building, projection, `models.loop`, and the `compactors.fail` path. Placement: last, because it consumes every active component.

</step-12>

Deferred contracts (task runtime, composition, compactor framework, host persistence, language and provider work) remain design documentation only; their tests, benchmarks, migrations, platform matrices, and acceptance scenarios do not block active work.

</execution-plan>
