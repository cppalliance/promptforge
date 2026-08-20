---
name: PromptForge review notes
overview: Running notes from the PromptForge source review. Observe only. Do not implement.
todos: []
isProject: false
---

# PromptForge review notes

Observe these notes. Do nothing else. Append new notes when they arrive. Do not turn this into a design or a task list.

1. Duplication between H1 execution (`execute_live_h1`) and H2 execution (`walk_section_blocks` / `run_one_section`). Same Lua/prose block loop shape, forked implementations.

2. Rename `walk_section_blocks` to `run_one_section_impl`. Keep wrapper `run_one_section`. Fanout calls that impl too. Slightly dirty on purpose: breadcrumb that this is not a third abstraction, it is the section walker's engine, reused. Better than a grand engine type. Stranger coming from fanout has to know the history. Accepted.

3. "Slightly dirty name" is the right phrase for that kind of breadcrumb. Observation for later naming, no action.

4. Review method: agent drives with A/B questions. Vinnie is the judgment engine. Decisions become refactor notes on this plan. No code until execute.

5. One block runner (A). H1 vs H2 differences are caller-set parameters / mode, not a second loop. Deviation stays visible at the call site. Illegal ops in a mode (example: `models.need` outside live H1) are runtime errors with a clear message, same family as other Lua errors. Do not grow a second language surface.

6. Do not split `engine.rs` on line count. ~1000 lines can be reasonable if it is the program guts. Inspect the file first. Split later only if jobs do not actually belong together.

7. Borrowed execute frames: prefer one struct, not Run/Walk/BlockWalk copies of the same fields. Differences should be unified at call sites if possible; leftover one-off fields become `Option` (fanout `item`). `ControlContext` stays a separate owned snapshot for Lua closures. Field-audit when we read the file; drop unread fields. Three parallel structs with mostly the same fields is a smell.

8. Preamble / prologue / epilog are **documentation terminology** for positions (H1; first Lua of a section; Lua after last prose). Behavior is emergent: missing model errors at first prose, epilog runs because it is the next block. Do not add phase types or extra enforcement. Keep the words in docs and comments so talk stays consistent. Comments must not imply a separate prologue/epilog engine.

9. `jump` transfers control. Target is a visible child or sibling. The jumper does not resume. After a child-level walk exhausts, the parent walk continues at the next sibling under normal fall-through / off-walk rules. Docs should say this. Keep the current engine behavior.

10. Children do not run by fall-through from the parent. Nesting is structure and an address space, not a call stack. Children run only via `jump` / `execute` / `fanout` (or other addressing). Docs should say that. Keep it.

11. Keep both `execute` (subroutine, nested walk, returns a string) and `jump` (goto, no return). Same walk rules inside an `execute` chain.

12. `fanout` is a core mechanic. Third driver of the same block runner (`run_one_section_impl`). Keep the share. Do not demote or isolate it.

13. One full tool loop per section (last prose). Earlier lua/prose is setup: progressive `tools.add`, store writes, then the final loop. Intended. Want another full loop: `execute` another section. Keep `loop_capable` on last prose only. Docs: setup then loop, not "cute last-block flag."

14. Keep off-walk. A leading `---` (first content of the section) takes the section off fall-through so helpers / lists / workers can live in the file. Addressed `jump` / `execute` / `fanout` still run it. A `---` after any prose or Lua block cuts the section: the rest is a comment (not compiled, not run). Two roles, one token. Docs must say both. Intended.

15. Keep `lua shared` in the preamble. One compiled library, replayed as the first chunk of every section VM. Not live H1. At most one fence; not under H1 as a second live program. Docs: shared helpers vs live preamble Lua.

16. `var` semantics (locked): the store is the intentional shared mutable world; `var` is the walk-local clipboard. One `var` per walk: fall-through and `jump` share it (H1 is the first writer, not a special seed). `execute()` and `fanout()` clone `var` in; child writes do not affect the caller. Store stays shared (execute/fanout writes are visible). Fixes docs (`promptforge.md`, `guide/src/lua.md`) vs implementation. Fanout arms keep fresh-per-arm clones.

17. `reply` carries to the next section on the same walk; `jump` preserves it unless the author sets `reply = nil`. Conversation does not carry. Keep. Docs already say this.

18. Progressive tool exposure (locked A): `tools.add` / `tools.add_local` between blocks reach the next prose. Setup turns stay narrow; last prose is the full loop. This is the small-model pattern. Keep.

19. `var` is JSON-only. Today a function in `var` fails at serialization (bad file/line). Refactor: guard `var` writes at assignment (proxy `__newindex`, same pattern as sealed `sys`) so non-JSON values fail at the Lua line that assigned them.

20. One model per section. `models.use` once, before first prose binds it. Do not switch models mid-section: same conversation, KV cache poisoned. `model:infer` is fine (fresh context). Want another model: `execute` or another section.

21. Capability binding in the preamble only (A). `tools.need` / `models.need` resolve when that Lua runs in H1. H2 uses captured bindings only. Fail fast: do not burn inference then discover a missing model in section 12. Keep.

22. `models.infer(prompt)` is the side-channel infer: one round, no tools, fresh conversation, does not touch `reply`. Default model is the section's current model. Must be able to pick another declared model (e.g. `models.get("tag"):infer(...)` or a two-arg form). Main model work stays in prose; this is for a narrow semantic question off the main thread. `model:infer` (in-context, tools, updates `reply`) is separate; do not conflate.

23. `sys` stays sealed read-only. Unknown field read or any write raises. Runtime facts are not author state. Keep.

24. Lua `return` stays scalar-only (string / integer / number / boolean). Tables are an error. Structured data goes in the store. Open design question for later: tables as JSON string on the Rust boundary, and prompt-from-prompt calls decoding JSON to a Lua table. Do not build without a concrete use case.

25. Keep `tools.always` in the preamble as the baseline for every model-facing section. `tools.add` is per-section on top. No pointless ceremony; frontier-model users can declare once up front. Keep.

26. Keep near-duplicate tool detection. Fail before the model sees two tools that read the same. Predictable behavior is the point of the language. Keep.

27. Keep `untrusted(s)` guard envelope for model-facing untrusted text (store reads, tool output, etc.). Required for handling hostile input. Keep.

28. Keep `max_tool_iterations`. Finite cap (can be large, e.g. 10000) prevents infinite tool loops. Configurable in frontmatter, runtime default otherwise. Keep.

29. Observer is a side channel for progress reporting. It cannot steer or cancel the run. Keep.

30. Keep `CancelHandle` for cooperative cancellation. Long runs need a kill switch; this is the mechanism. Keep.

31. Keep `DebugCapture` as opt-in raw request/response hook. Production leaves it unset. Debugging needs the wire. Keep.

32. Keep `execute()` recursion cap at 8. `fanout()` shares the same cap and counter (nested fanout counts against the same depth). 8 is deep enough; runaway recursion is a bug. Keep.

33. Keep `tools.calls` per-section counts. Needed so a prompt can require a small model to call a tool at least N times (e.g. `assert(tools.calls.search >= 1)`). Observer sees the calls; `tools.calls` is the in-language gate. Keep.

34. Keep `models.default` in the preamble as the prompt-wide baseline. `models.use` overrides per section. One-model users should not repeat themselves. Keep.

35. Keep frozen Model handles from `models.need`. Read-only after capture. No values to change mid-run. Keep.

36. Frozen Tool objects. No mutable `.description`. Description override is a positional string parameter: `tools.need(alias, catalog_desc, override?)`, `tools.always(alias, override?)`, `tools.add(alias, override?)`. Precedence: `add` > `need`/`always` > catalog. Named table only if more overrides appear later. Change.

37. Keep `tools.add_local`. Lua-backed tools are the structured-data capture mechanism (the alternative to Pydantic). The model calls a tool, the handler runs in the section VM, the caller gets structured arguments. Core value. Keep.

38. Keep the store virtual and run-scoped. No real filesystem access through `store.*`. If real FS access is ever added, it is a different shape: `ls`, `grep`, `stat`, `mkdir`, etc. as separate tools, not store paths. Keep.

39. Keep `log()` in some form. Need a way to dump internal state for prompt debugging. Whether it goes through the observer or a separate channel is open. The 256-char / no-newline cap may be too tight for state dumps. Revisit when we know what debugging actually needs.

40. Prose substitution namespaces: `args`, `reply`, `var`, `sys`, plus bare globals (section-local). Five sources. No more. Keep.

41. Keep `{{ item }}` in fanout arms. Arms need to distinguish each other and carry arm-specific data. `item` is that mechanism. Keep.

42. Drop the `tasks` table. Strings only for `execute` / `jump` / `fanout` targets. `tasks["## Research"]` is a handle around a string with no added value. If a lookup table ever returns, name it `sections`, not `tasks`. Change.

43. Keep `list_from_section`. Markdown lists are the natural way to write a collection. Building the same array in Lua is ugly (no formatting, no bullets). List sections feed `fanout`. Keep.

44. Keep `execute(target, input?)`. Subroutines take an `args` string, same as the prompt itself. The input becomes the subroutine's `args`. Symmetry with the top-level run. Keep.

45. Keep fanout results as objects with `.text`, `.ok`, `.item`, `.exhausted`. `__tostring` coerces to `.text` for `tostring` / `table.concat`. Already an object; no reason to strip it. Keep.

46. `sys.id` should be a global counter across all chains. Every unit of execution (section, fanout arm, execute chain) gets its own unique small integer. Entering the same section twice gives two IDs. Change from per-chain counting.

47. `var` persists across sections on the same walk (transport). H1 is on the same walk as top-level H2, so H1 `var` writes persist into H2. `execute` and `fanout` start new walks with cloned `var`. Bare globals (`x = 42` without `local`) are section-local and visible to `{{ x }}` in prose. No new keyword, no new table. `var` = transport, globals = scratch. Change.

48. Guard `var` writes at assignment (JSON-only). Bare globals are unguarded scratch; substituting a non-string global is the author's bug. `sys.when` and `sys.now` both stay. `sys.model` and `sys.reply_finish_reason` stay lazy (appear when known). `store.read` on missing path is an error; `store.exists` returns boolean. Keep.

49. Keep `store.str_replace` unique-anchor semantics. Fails if not found or found more than once. Predictable. Keep.

50. Keep `store.glob` with glob patterns (`*`, `**`, `?`). Enough for now. Can add regex later if needed. Keep.

51. Keep both `store.read` (verbatim) and `store.read_numbered` (with line numbers). Numbered is for provenance: any tool that reports where data came from by line number. Not just editing. Keep.

52. Keep the store operation set minimal: write, append, read, read_numbered, str_replace, delete, glob, exists. No rename, copy, or partial write. Add later if needed. Keep.

53. Store paths create parents implicitly. No `mkdir` ceremony. Keep.

54. `store.str_replace` on a missing file is an error. You cannot edit what is not there. Keep.

55. `store.delete` on a missing file is silent. Idempotent delete. Change.

56. `store.exists` and `store.glob` never error. Queries return boolean or empty array. Keep.

57. `store.read` and `store.read_numbered` with line range clamp to file bounds. Reading past EOF gives what exists. Models are approximate; do not error on out-of-range. Keep.

58. `store.write` with empty content creates an empty file. Valid state. Prevents errors later when something tries to read it. Keep.

59. `store.append` to a missing file creates it. Required for fanout: all arms can append without checking `exists` first. No ceremony. Keep.

60. `store.append` with empty content is a no-op. Fine. Keep.

61. `jump` inside a local tool handler: allowed in principle (models orchestrate through tool calls), but likely hard to implement. Workaround: save the jump target, invoke it after the tool returns. Keep `jump` nilled for now. Revisit if a real prompt needs it.

62. Cap `fanout` concurrency. Limit can be high (e.g. 1000) but must be finite. Prevents runaway resource use. Change.

63. Fanout arm failure: hard errors (Lua, store, model) abort siblings. Soft exhaustion (hit `max_tool_iterations`) lets siblings finish; the failed arm returns `.ok = false`, `.exhausted = true`. Keep.

64. `fanout` results are in collection order. `results[i]` corresponds to `items[i]`. Index each arm consistently. Keep.

65. `fanout` with an empty collection is an error. No work is likely a bug. Revisit if a real use case appears. Change.

66. `fanout` worker with no prose (Lua-only) is allowed. Valid for compute / store writes with no model. Keep.

67. `fanout` worker that falls through with no reply: `.text` is empty string. `"done"` would be a lie. Caller needs accurate information. Change.

68. `fanout` worker that jumps: allowed. Jump starts a child walk, same as a section. No special cases. Uniform behavior. Keep.

69. `fanout` worker that calls `execute`: allowed. Subroutines are what makes computation powerful. Max depth limit (8, shared with `execute`) still applies. Keep.

70. `fanout` worker that calls `fanout`: allowed. Same principle: uniform behavior, no special cases, max depth limit. Keep.

71. `fanout` worker that calls `model:infer` or `models.infer`: allowed. Both are fresh context, no tools, no `reply` touch. Uniform with regular sections. Keep.

72. `fanout` worker that calls `tools.add` or `tools.add_local`: allowed. Arms start from scratch (H1 `tools.always` baseline only). They do not inherit the caller's tool bag. Keep.

73. `fanout` worker that calls `models.use`: allowed. Arms pick their own model from the captured bindings. Symmetric with sections. Keep.

74. `fanout` worker that calls `store.write` / `store.append`: shared store. All arms see the same store. Keep. Consider: two arms calling `write` on the same path is a hard error (write-write race). Append is fine (order unspecified). Open.

75. `fanout` worker that calls `log`: allowed. Goes to the observer with the arm's section name. Symmetry principle. Keep.

76. `fanout` worker that calls `untrusted`: allowed. Pure function. Keep.

77. `fanout` worker that calls `list_from_section`: allowed. Symmetry. Keep.

78. Nested fanout inside `execute` inside an arm: allowed. Depth cap is the only limit. Symmetry. Keep.

79. `fanout` worker that calls `models.default`: error. `default` is preamble-only. Arms use `models.use` to override. Keep.

80. `fanout` worker that calls `tools.need` or `models.need`: error. `need` is preamble-only (live resolution). Arms use captured bindings. Keep.

81. Arms are sections plus `item`. Same capabilities, no special cases. `item` is the arm's identity (the collection member). Everything else is uniform. Keep.

82. Add `sys.index`: 1-based arm index within the current fanout. Nested fanout starts from 1. Absent outside fanout. `sys.id` is the global execution unit ID (note 46). `sys.index` is the arm position. Different questions. Change.

83. One `infer` shape only: one round, no tools, fresh conversation, never sets `reply`, never touches `sys`. Two forms: `models.infer(prompt)` (the section's current model) and `models.get(alias):infer(prompt)` (any declared model's handle). The tool-loop `model:infer` is deleted outright - tools advertised, `reply`/`sys.reply_finish_reason` updates, `ToolBag` and its generation cache all go; it was never a wanted capability. Supersedes the "separate; do not conflate" half of note 22 and matches note 71's description of both forms. A Lua block that needs tools uses `execute` on a section. Change.

84. Note 17's `reply = nil` escape is not implemented. The walk carries `reply` as a Rust local seeded only by prose outcomes; nothing reads the Lua `reply` global back, so a Lua `reply = nil` (or any Lua write to `reply`) before a jump or fall-through does not change what the next section sees. The docs promise the clear (`guide/src/lua.md`, both user guides, `design-core.md`). Either read the global back at section end or drop the promise from the docs. Change.

85. Fanout arms seed `reply` from the fanout caller's section-start incoming reply (captured once in `make_control_globals`), not the caller's current reply at the call site: a caller whose own prose produced a reply before calling `fanout` hands every arm the stale pre-section reply. `execute` chains seed nil instead. No note or doc covered arm reply seeding. Keep the seed (an arm is its own walk and inherits the walk's incoming context); docs must state it. Keep.

86. Note 83 naming residue. `engine.rs`, `arm.rs`, and `support.rs` comments still say "the `model:infer` hook" and list `model:infer` as a bridged host call; `guide/src/lua.md`'s local-tools section still tells authors a handler may call `model:infer`; `design-core.md` still describes in-context `model:infer` accumulating conversation, and still says `jump` inside `execute()` is a hard error (superseded by note 11: same walk rules inside a chain). The kept forms are `models.infer(prompt)` and `handle:infer(prompt)`. Comments and stale docs should use the note-83 names. Change.
