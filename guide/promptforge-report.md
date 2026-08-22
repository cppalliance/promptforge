# PromptForge: A Prompting Language

## Executive Summary

PromptForge is best understood not as a prompt template format, nor merely as Markdown with embedded Lua, but as a **capability-scoped orchestration language for model computation**.

Its central design achievement is the separation of concerns between deterministic execution and semantic judgment. Markdown prose expresses work for a model. Lua governs deterministic control flow. Sections provide explicit isolation boundaries. Models and tools are declared as capabilities rather than assumed as ambient authority. State crosses boundaries only through named mechanisms. When deterministic logic is insufficient, a model can be recruited for a narrow semantic judgment; when a sufficiently capable model is available, the prompt author may go further and selectively delegate orchestration itself.

This produces an unusually coherent hierarchy of authority:

- **Lua owns deterministic orchestration by default.**
- **Models own semantic inference by default.**
- **`model:infer()` lets deterministic code ask a small semantic question.**
- **Prose blocks perform substantive model work.**
- **Tool scoping determines what actions a model may take during that work.**
- **Orchestration operations can themselves be exposed as tools, allowing capable models to share control when the author explicitly permits it.**

That last point is particularly important. In PromptForge, orchestration is not an all-or-nothing architectural choice. A 3B model can be kept on an extremely short leash: one narrow task, one or two tools, deterministic transitions controlled by Lua. A 70B-class model can be given a larger action space, including model-facing orchestration tools such as `jump()`, `task()`, and `fanout()`. The language does not dictate how much authority a model deserves. It supplies the mechanism by which the prompt author grants that authority deliberately.

PromptForge therefore avoids a weakness common in agent frameworks: making the language model simultaneously understand the problem, discover the workflow, select among many tools, maintain state, and decide when it is finished. PromptForge can assign each of those responsibilities to the most suitable mechanism.

The result is a system with a clear thesis:

> **The model decides semantic answers; the program decides what the model is allowed to do, when it does it, what context it receives, and where its output goes—unless the author deliberately delegates some of those decisions back to the model.**

This is a strong foundation for both small-model reliability and large-model agency.

---

## 1. The Core Architectural Idea

PromptForge treats model inference as a computational primitive inside a larger deterministic program.

That distinction is more consequential than it first appears.

Many so-called agent systems begin by giving a model a collection of tools and a broad objective:

> Here are the capabilities. Determine the plan, choose the tools, perform the work, recover from mistakes, and decide when you are done.

PromptForge permits that style when desired, but it does not require it. The author can instead write the execution graph explicitly:

```text
classify intent
→ choose source policy
→ search
→ inspect results
→ fetch
→ synthesize
→ validate
→ return
```

The model performs the portions that require language understanding, synthesis, judgment, or interpretation. Lua performs the portions that are straightforward program logic.

This arrangement moves uncertainty into the places where uncertainty is useful.

A deterministic condition should remain deterministic. A semantic question should be answered semantically. A model should not have to rediscover a workflow that the prompt author already knows.

That is PromptForge's most important design decision.

---

## 2. Capability Scoping Is More Than Tool Management

The strongest practical feature in PromptForge may be its treatment of tools as **scoped capabilities**.

Declaring a tool with:

```lua
tools.bind("search", "Search the web and return a list of results.")
```

does not automatically expose that tool to the model.

Exposure is a separate act:

```lua
tools.add("search")
```

This distinction is excellent.

It means the runtime can know that a prompt depends on a capability without implying that every model turn should see that capability. The author decides when the capability becomes part of the model's action space.

For a large model, this is useful discipline.

For a small model, it can be essential.

A 3B-parameter model asked to choose among many vaguely related tools is solving two problems at once:

1. What does the user want?
2. Which of these many mechanisms should I invoke?

PromptForge allows the author to remove the second problem whenever it is unnecessary.

A search phase can expose only search. A later phase can expose fetch. A reporting phase can expose neither. The model sees only the actions relevant to the immediate decision.

This is not merely schema reduction. It is **cognitive environment design**.

PromptForge lets the author constrain the model's visible action space in the same way that a well-designed API constrains the operations available to ordinary code.

### Progressive capability exposure

The multi-step section model is therefore essential.

A section can begin with one capability, perform an inference, then expose another capability for a later inference while preserving the surrounding program's intent.

Conceptually:

```lua
tools.add(search)
writer:infer("Identify the best sources for this request.")

-- inspect or use the result

tools.add(fetch)
writer:infer("Fetch and evaluate the selected sources.")
```

The author does not have to present `search`, `fetch`, private-source access, orchestration tools, and every other possible capability simultaneously.

This gives PromptForge a powerful optimization target:

> **Expose the minimum sufficient capability set for each semantic decision.**

That principle deserves to be one of the language's advertised strengths.

---

## 3. Authority Is Explicit and Transferable

PromptForge's capability model extends naturally from ordinary tools to orchestration itself.

Searching is a capability.

Fetching is a capability.

Accessing private MCP sources is a capability.

Parallel execution is a capability.

Calling another task is a capability.

Changing control flow is a capability.

This yields an elegant authority model.

Lua begins with orchestration authority because Lua is the deterministic control language. Models begin with inference authority because models are recruited to make semantic judgments and produce semantic output.

But the prompt author may selectively give a model additional authority by exposing orchestration operations as tools.

The model can therefore remain a worker:

```text
Lua chooses the section.
Lua chooses the tools.
Model answers the question.
```

Or it can become a partial orchestrator:

```text
Lua establishes the environment.
Model chooses among permitted orchestration operations.
Runtime enforces the resulting action.
```

This is particularly attractive because the language does not need a separate "agent mode." The same capability mechanism governs both ordinary tools and control-flow tools.

The difference between a constrained worker and an autonomous agent is therefore not a separate runtime architecture. It is a difference in the capabilities placed in scope.

### A natural model-size gradient

This architecture maps well onto model capability.

For a small model:

```text
few tools
narrow prose
deterministic Lua control
little or no orchestration authority
```

For a stronger model:

```text
larger semantic tasks
more tools
selective orchestration authority
possibly dynamic task selection or fanout
```

For a very strong model, the author may choose to expose tools corresponding to `jump()`, `task()`, or `fanout()` and allow the model to construct portions of the execution graph dynamically.

The important design principle is that this is a **policy decision**, not a language requirement.

PromptForge separates mechanism from policy unusually well.

---

## 4. Sections Are Execution Boundaries, Not Merely Headings

PromptForge's Markdown structure is doing real semantic work.

The H1 identifies the prompt and hosts shared declarations. H2 sections are executable units. H3 sections have specialized roles such as fanout workers and lists.

This is a good use of Markdown because the visual organization matches the runtime organization.

A section is not just a labeled fragment of prompt text. It is an isolation boundary.

Each section receives:

- a fresh Lua VM;
- a fresh model conversation;
- its own `var`;
- access to run-scoped facilities such as the store;
- the appropriate `reply` state for normal sequential flow.

This is a disciplined model.

It prevents accidental persistence of Lua state and accidental persistence of conversation history. Information survives only when the program deliberately places it into a channel intended to survive.

That makes section boundaries meaningful both operationally and cognitively.

The model sees a fresh context.

The programmer sees a fresh VM.

The runtime sees a new execution unit.

This is exactly the kind of invariant that makes a language easier to reason about as it grows.

---

## 5. The State Model Is Explicit and Coherent

PromptForge has several kinds of state, but they serve distinct purposes.

### Lua state

Lua locals and globals exist inside the current VM.

They are ordinary computation state. They are not a cross-section persistence mechanism.

Shared preamble code may reconstruct global helpers or bound handles when a fresh section VM is created, but runtime state from a previous section does not silently leak across that boundary.

That is a good invariant:

> **Nothing in the Lua VM propagates implicitly.**

### `var`

`var` is the section's prompt-facing state table.

It exists so Lua can compute values that are later substituted into prose:

```lua
var.topic = args
var.count = 3
```

followed by:

```text
Write {{ var.count }} facts about {{ var.topic }}.
```

Because every section receives a fresh VM and fresh `var`, `var` is not a hidden persistence mechanism. It belongs to the current section execution.

That is exactly how it should work.

### `reply`

`reply` has a simple invariant:

> **`reply` is the model's response.**

After a prose turn, it contains the resulting model text.

After `model:infer()`, it contains that model response.

On ordinary section fall-through, the preceding model response is available to the next section as `reply`.

This is not semantic overloading. It is one concept preserved across execution contexts: the most recent model-generated response available to the current execution.

### `store`

The store is the explicit mechanism for persistent program state across execution boundaries.

It is run-scoped and shared across sections, `execute()` calls, and fanout arms.

The distinction can be summarized compactly:

```text
Lua state    = computation inside one VM
var          = prompt-facing state inside one section
reply        = model response
store        = persistent program state across contexts
```

This is a strong state model because different lifetimes are visible in the source.

If a value belongs only to the current computation, use Lua.

If it must appear in the current section's prose, use `var`.

If it is a model response, it appears in `reply`.

If it must survive a fresh execution context, put it in the store.

There is very little hidden magic.

---

## 6. `model:infer()` Is Semantic Computation from Lua

`model:infer()` is best understood as an **impromptu semantic query** made from deterministic code.

Its purpose is not primarily to replace prose blocks or to become the ordinary way of building a model workflow.

Its purpose is to answer questions that Lua cannot reasonably answer.

For example:

```lua
local answer = writer:infer(
    "Does this request imply that the user wants to avoid searching "
    .. "private MCP sources? Answer only yes or no.\n\n"
    .. args)
```

Lua can then act on that judgment:

```lua
if answer == "yes" then
    -- do not expose private-source capabilities
else
    tools.add(private_search)
end
```

This is a valuable middle ground between deterministic programming and full model autonomy.

The program does not hand control flow to the model. It asks the model one narrow semantic question and resumes deterministic execution with the answer.

Useful cases include:

- intent classification;
- ambiguity resolution;
- choosing a policy path;
- determining whether an input belongs to a semantic category;
- deciding which capabilities should be exposed;
- extracting a small fact needed by subsequent Lua logic;
- selecting among a few strategies when the distinction is semantic rather than syntactic.

This makes the architecture unusually expressive without requiring many new language constructs.

Lua handles what Lua is good at.

The model handles what language models are good at.

`model:infer()` is the bridge between them.

---

## 7. Prose Blocks Are the Primary Model Workflow

If `model:infer()` is an impromptu semantic query, prose remains the natural expression of substantive model work.

This distinction is important because it keeps the source readable.

Prompt authors should be able to open a PromptForge file and see, in ordinary language, what each model is being asked to do.

For example:

```markdown
## Report

Using only the evidence below, write a structured briefing.

{{ var.evidence }}
```

This is better than burying every model interaction inside Lua strings.

Markdown remains the language of semantic work; Lua remains the language of orchestration.

Alternating Lua and prose blocks then support multi-turn interactions inside a section without discarding that readability.

A useful conceptual distinction is:

```text
model:infer()
    semantic question asked by the program

prose block
    substantive semantic work requested by the program
```

This is much clearer than treating both merely as two spellings of "call the model."

---

## 8. `jump()` Is `goto`, and That Is Fine

`jump()` should be described plainly.

It is the PromptForge control-transfer operation analogous to `goto`.

The name is `jump` because `goto` is reserved in Lua.

There is no need to apologize for this.

In a workflow language, transferring execution to a named section is a legitimate primitive:

```lua
if approved then
    jump("## Accept")
else
    jump("## Reject")
end
```

Lua makes the decision. `jump()` performs the transfer.

This is different from calling a subroutine.

The distinction matters:

```text
jump()      execution continues elsewhere
execute()   perform another section and return its result
```

Trying to model every branch as:

```lua
return execute("## Accept")
```

would obscure the actual intent. It would turn control transfer into a fake function call.

`jump()` is therefore not redundant with `execute()`.

It is the correct primitive for straightforward conditional orchestration.

The language benefits from stating this without euphemism:

> **`jump()` is a section-level `goto`. Lua's `goto` keyword is reserved, so PromptForge uses the name `jump`.**

That sentence probably eliminates more confusion than a page of abstraction terminology.

---

## 9. `execute()` Makes Sections Into Subroutines

`execute()` provides the complementary control operation.

Where `jump()` transfers control, `execute()` invokes another section and returns.

That means a section can act like a prompt subroutine:

```lua
local evidence = execute("## Research")
local report = execute("## Report")
return report
```

The called section receives a fresh VM and fresh model conversation while sharing the run's persistent facilities.

This is an excellent abstraction.

A prompt subroutine should not accidentally inherit the caller's transient Lua variables or conversational residue. It should receive an explicit input, perform a bounded semantic operation, and return a result.

The separation between `jump()` and `execute()` is therefore clean:

| Operation | Meaning |
|---|---|
| `jump()` | transfer control |
| `execute()` | call a section and return |
| normal fall-through | continue to the next section |

Those three operations are enough to express a substantial amount of workflow structure without inventing a separate workflow syntax.

Lua remains the control language.

Sections remain the units of semantic computation.

---

## 10. `fanout()` Is a High-Value Primitive

`fanout()` provides parallel semantic execution without forcing the author to program an asynchronous runtime.

Its surface syntax is intentionally small:

```lua
local results = fanout("### Worker", "### Topics")
```

Yet the operation provides:

- one worker execution per item;
- fresh execution context per arm;
- access to the current item;
- concurrent execution;
- deterministic result ordering;
- shared run-scoped store access;
- structured result metadata.

This is exactly the sort of operation worth elevating into the language.

The author describes *what is parallelizable*, not the machinery required to schedule it.

A useful design test for language primitives is whether they collapse a large amount of incidental machinery into an operation with clear semantics.

`fanout()` passes that test.

It is also a natural candidate for selective model orchestration. A strong model may be capable of deciding when a problem should be decomposed and parallelized, provided the author deliberately exposes that authority.

Again, the underlying operation need not change. Only the party choosing to invoke it changes.

---

## 11. Model and Tool Binding Resembles Dependency Injection

PromptForge does not require prompt files to hard-code concrete infrastructure identities.

Instead, they can declare semantic requirements:

```lua
writer = models.bind(
    "writer",
    "A careful analysis model",
    { thinking = false, temperature = 0, context = 32768 })
```

and:

```lua
search = tools.bind(
    "search",
    "Search the web and return a list of results.")
```

The runtime resolves those requirements against the available catalog.

This resembles dependency injection more than conventional prompt configuration.

The prompt declares what it needs.

The environment determines what concrete resource satisfies that need.

That has several benefits:

- prompts are less coupled to providers;
- the runtime can choose different implementations in different environments;
- capability requirements are visible in one place;
- resource binding can be validated before execution;
- prompt logic can remain stable while infrastructure evolves.

This is one of PromptForge's more distinctive architectural features and deserves stronger emphasis in how the language is described.

A useful formulation is:

> **A PromptForge program declares resource requirements and receives bound capabilities.**

That is more powerful than saying merely that PromptForge supports multiple models and tools.

---

## 12. Simple Prompts Remain Simple

A language with this much orchestration machinery could easily become unpleasant for ordinary use.

PromptForge avoids that trap.

A minimal prompt remains approximately:

```markdown
---
name: summarize
description: Summarize the input.
promptforge: 1
---

# Summarize

## Main

Summarize the following text in one sentence:

{{ args }}
```

The author pays for advanced features only when advanced features are needed.

Lua is optional.

The store is optional.

Multiple sections are optional.

Fanout is optional.

Explicit inference is optional.

Tool scoping is optional when no tools are involved.

That progressive complexity is a major strength.

The best domain-specific languages often have this property: the trivial case looks like the domain, not like the machinery.

PromptForge's trivial case looks like a prompt.

That is exactly right.

---

## 13. Template Substitution Has a Clean Boundary

The substitution vocabulary is small and comprehensible:

```text
{{ args }}
{{ reply }}
{{ var.x }}
{{ sys.when }}
{{ item }}
```

More importantly, substitution applies to prose, not Lua source.

That boundary should remain firm.

Allowing template expansion inside Lua would create a mixed-language quoting problem and make it much harder to reason about whether a value is code, data, or prompt text.

The present division is much safer:

> Lua is Lua.  
> Prompt text is templated.

Hard errors for missing substitution paths are also the right choice. A typo such as:

```text
{{ var.nmae }}
```

should not silently become an empty string and produce a mysterious model failure later.

PromptForge generally favors explicit failure over silent degradation, and that is a good language-design instinct.

---

## 14. The Store Encodes a Security Boundary

The distinction between:

```lua
store.read(path)
```

and:

```lua
store.inject(path)
```

is particularly thoughtful.

`store.read()` returns raw data to Lua.

`store.inject()` prepares stored material for model-facing use inside an explicitly untrusted envelope.

That is not merely convenience. It encodes an important security idea into the language:

> **Data that is safe for deterministic code to inspect is not automatically trusted as model instruction.**

This resembles the distinction between concatenating strings into a database query and binding data as parameters.

The general idea may eventually deserve to grow beyond the store. External tool results, user files, fetched pages, MCP content, and other sources may all benefit from a common notion of "model-facing untrusted data."

But the current mechanism should not be generalized merely for symmetry. The important thing is to preserve the security principle and extend it only when concrete use cases justify the additional abstraction.

---

## 15. Validation Belongs in the Program

PromptForge permits Lua after model work to inspect the result and determine whether execution should continue.

This is another important separation of responsibilities.

A model can be asked to produce sourced research.

Lua can then verify that required tools were actually invoked, required artifacts were produced, or obvious output constraints were met.

For example:

```lua
local searches = tools.calls["search"] or 0
local fetches = tools.calls["fetch"] or 0

if searches == 0 or fetches == 0 then
    return "INCOMPLETE"
end
```

This is better than asking the model to certify its own behavior.

Where an invariant can be checked mechanically, the program should check it mechanically.

PromptForge's use of hard errors for malformed programs and explicit quality gates for model output creates a promising distinction:

```text
programming/infrastructure failure → error
semantic/model-quality failure     → inspectable program state or result
```

That distinction should eventually be made explicit in the language specification so authors can predict which failures abort execution and which are intended to be handled.

---

## 16. The Language Has a Small Number of Powerful Control Concepts

One of PromptForge's greatest risks from this point forward is not missing capability.

It is feature accretion.

The language already has enough primitives to describe sophisticated systems:

```text
prose
Lua
model:infer()
jump()
execute()
fanout()
var
reply
store
tool scoping
model selection
validation
runtime metadata
```

Lua already provides:

- conditions;
- loops;
- functions;
- tables;
- string manipulation;
- ordinary local computation.

PromptForge should resist adding a specialized DSL construct whenever ordinary Lua plus the existing semantic primitives can express the same behavior clearly.

A useful admission test for every proposed feature is:

> **Does this operation represent a genuinely model-specific or execution-context-specific concept, or is it merely general-purpose programming that Lua already expresses?**

`fanout()` earns its place because parallel prompt execution is a runtime concept that would otherwise require substantial machinery.

`jump()` earns its place because section-level control transfer is a PromptForge runtime operation.

`execute()` earns its place because fresh-context section invocation is a PromptForge runtime operation.

A special syntax for ordinary branching would not earn its place, because Lua already has `if`.

This discipline will matter more as PromptForge becomes successful.

Languages rarely collapse because they cannot add features. They become difficult because they cannot refuse them.

---

## 17. The Mental Model Can Be Made Very Compact

A mature language benefits from an explanation that fits in a few sentences.

PromptForge now has one.

> **Markdown expresses model work. Lua expresses deterministic control. Sections define execution boundaries. `var` carries section-local prompt state, `reply` carries model output, and `store` carries persistent run state. Tools grant capabilities to models. `model:infer()` lets Lua ask a semantic question. `jump()`, `execute()`, and `fanout()` move execution through the prompt graph. Those same orchestration capabilities may be exposed to sufficiently capable models when the author wants to share control.**

There is also a useful four-line version:

```text
Lua                 deterministic reasoning
model:infer()       semantic reasoning used by the program
prose               semantic work requested by the program
tools               authority available to semantic work
```

And an equally useful state version:

```text
Lua state           one VM
var                  one section
reply                model response
store                one run
```

These compact models are evidence that the language has a coherent center.

---

## 18. What Makes PromptForge Different From an Agent Framework

The word "agent" often conceals several independent design decisions:

- Who chooses the next action?
- Who selects the tools?
- Who decides whether context should persist?
- Who chooses what state survives?
- Who decomposes the work?
- Who decides when parallelism is useful?
- Who determines whether external sources are allowed?
- Who validates the result?
- Who decides when execution is complete?

Many agent frameworks answer all of these with the same word:

> the model.

PromptForge does not.

It lets the prompt author assign each responsibility deliberately.

A small model can be treated as a specialized semantic function inside a deterministic program.

A larger model can be given a wider tool vocabulary.

A highly capable model can be granted orchestration operations.

The architecture therefore supports a continuum:

```text
model as function
→ model as tool-using worker
→ model as constrained planner
→ model as partial orchestrator
```

without changing the fundamental execution model.

This is a significant advantage.

It means PromptForge can scale *with* model capability instead of assuming that every model should be treated as an autonomous agent.

---

## 19. The Language's Deepest Design Principle

The individual features—Markdown sections, Lua, store, tool binding, fanout—are useful, but they are not what makes the design compelling.

The deeper principle is **explicit authority at explicit boundaries**.

A model does not receive a tool because the tool exists.

A section does not inherit state because another section happened to create it.

A subtask does not inherit conversation simply because it was called.

A value is not inserted into model-facing text merely because Lua can read it.

A model does not control execution merely because it is capable of producing tool calls.

Each transfer is explicit:

```text
expose this tool
inject this data
persist this state
call this section
jump to this section
ask the model this question
delegate this orchestration capability
```

That explicitness is what gives PromptForge a chance to remain understandable even when the workflows become complex.

It also makes the language suitable for a world in which model capability varies dramatically.

The same program can keep a small model constrained and give a larger model room to maneuver.

The runtime does not have to guess which behavior is appropriate.

The author states the policy.

---

## 20. Actionable Suggestions

- **State the language's thesis near the beginning of the guide.** Describe PromptForge as a capability-scoped orchestration language in which deterministic code and language models can share control. This framing explains the rest of the design better than "Markdown prompts with Lua."

- **Elevate capability narrowing to a first-class design principle.** Explain explicitly that `tools.add()` is not merely a way to make tools available; it lets the author minimize the model's action space at each turn, which is especially valuable for small models.

- **Explain progressive tool exposure with a small-model example.** Show a 3B-class workflow in which a first inference sees only `search`, a later inference gains `fetch`, and private or orchestration capabilities remain absent unless needed.

- **Describe tools as grants of authority.** Make clear that `tools.bind()` declares a dependency while exposure determines what the model is actually allowed to do. Extend that explanation naturally to model-facing orchestration tools.

- **Document the orchestration ladder.** A concise progression such as `Lua → model:infer() → tool-using prose → model-facing orchestration tools` would show authors how to escalate from deterministic control to delegated agency only as necessary.

- **State plainly that `jump()` is `goto`.** Suggested wording: "`jump()` is a section-level control transfer analogous to `goto`; the name `jump` is used because `goto` is reserved in Lua." This is clearer and more memorable than abstract descriptions of context transfer.

- **Teach `jump()` and `execute()` as a deliberate pair.** "`jump()` transfers; `execute()` calls and returns." The distinction is fundamental and should appear early in the control-flow documentation.

- **Frame `model:infer()` as impromptu semantic computation from Lua.** Its defining use case is not ordinary workflow authoring but resolving a fuzzy question that deterministic code cannot answer conveniently: intent, classification, policy selection, routing, or another small semantic judgment.

- **Keep substantive model work in prose by convention.** Encourage authors to reserve `model:infer()` for compact semantic questions and use prose blocks for the main work. This preserves PromptForge's readability as Markdown rather than gradually turning prompts into Lua programs containing long string literals.

- **Clarify the two kinds of multi-turn behavior.** Explain that explicit inference can be used while capabilities are still being assembled, whereas alternating prose blocks represent the visible semantic conversation under the section's established scope. Teach the reason, not only the parser rule.

- **Publish the state-lifetime table prominently.** Use a compact table or diagram:
  - Lua state — current VM
  - `var` — current section
  - `reply` — model response
  - `store` — current run  
  This makes the fresh-VM semantics immediately intuitive.

- **Describe `var` as model-facing section state, not merely a variable table.** Its important property is that Lua can prepare values for safe, explicit template substitution within the current section.

- **State the `reply` invariant in one sentence.** "`reply` is the model's response." Then document when it is replaced, propagated, or intentionally cleared. Avoid introducing extra terminology that makes this simple concept appear overloaded.

- **State the fresh-context invariant prominently.** "Nothing in the Lua VM propagates implicitly across a fresh section execution." This is one of the language's strongest reasoning guarantees.

- **Describe the store as the cross-context persistence mechanism.** Authors should immediately understand that values which must survive a fresh VM or conversation boundary belong in the store.

- **Elevate `store.inject()` as a security feature.** Explain the principle behind it: data readable by deterministic code is not automatically trusted as model instruction. This is more important than presenting `inject()` merely as another read operation.

- **Consider, later, a general model-facing untrusted-value abstraction.** Tool results, fetched documents, MCP content, files, and store data share a security concern. Do not add this abstraction until concrete use cases justify it, but preserve the conceptual path.

- **Formalize the failure taxonomy.** Distinguish malformed-program or infrastructure failures, which should abort, from semantic/model-quality failures, which authors may inspect and handle. Fanout exhaustion and validation gates will be easier to reason about once this rule is explicit.

- **Keep hard errors for missing template paths and illegal scope operations.** Silent substitution failure, unknown tool aliases, or accidental out-of-scope calls would make debugging model behavior unnecessarily mysterious.

- **Emphasize that model and tool declarations are dependency injection.** "The prompt declares what capability it needs; the runtime binds an implementation." This is a strong architectural story and differentiates PromptForge from provider-specific prompt formats.

- **Preserve the rule that simple prompts remain simple.** Advanced orchestration should continue to be opt-in. A one-turn prompt should always look like a one-turn prompt.

- **Protect Markdown as executable structure.** H1, H2, and H3 already have semantic roles. Document that openly: Markdown hierarchy is part of the language grammar, not decorative formatting.

- **Avoid template substitution inside Lua.** The current boundary—Lua is code, prose is templated—is clean, safe, and easy to reason about.

- **Keep orchestration as capability, not identity.** A model should not automatically become an agent because it is large. It should become an orchestrator only when the author exposes the relevant operations.

- **Document model-size guidance as policy, not a runtime rule.** Small models generally benefit from narrow tool sets and deterministic orchestration; stronger models may successfully use broader tool sets and orchestration capabilities. The language should enable both without hard-coding a parameter-count doctrine.

- **Provide side-by-side "small model" and "large model" examples.** Use the same problem twice: first with Lua controlling every transition and a narrow tool set, then with a strong model given selected orchestration tools. This would demonstrate PromptForge's architecture better than an abstract feature list.

- **Consider naming the concept of delegated orchestration.** A term such as *orchestration authority* or *control capability* would give documentation a stable way to discuss the difference between allowing a model to search and allowing it to alter the execution graph.

- **Keep `fanout()` primitive and high-level.** Do not expose ordinary asynchronous machinery to prompt authors unless a compelling use case appears. Its strength is that it expresses semantic parallelism without making authors program a scheduler.

- **Resist adding DSL syntax for things Lua already does well.** Conditions, loops, tables, local functions, and ordinary computation belong in Lua. PromptForge syntax should be reserved for model turns, context boundaries, capability management, persistence, and orchestration operations that Lua alone cannot implement.

- **Use a strict admission test for future features.** Ask: "Does this represent a genuinely PromptForge-specific semantic or execution concept?" If the answer is no, it probably belongs in Lua or in a library rather than in the language.

- **Consider an explicit terminology section for execution boundaries.** Terms such as *run*, *section execution*, *conversation*, *VM*, *fanout arm*, and *subroutine execution* are all meaningful. Defining them once will prevent later documentation from describing the same boundary in subtly different ways.

- **Make the capability-security analogy explicit.** The same principle governs ordinary tools, private MCP access, and orchestration: possession by the runtime does not imply authority for the model. Exposure is the grant.

- **Describe model-facing `jump()`, `task()`, and `fanout()` as the same underlying operations under a different decision-maker.** Lua invocation means deterministic code chose the action; tool invocation means the model chose among permissions the author granted. This is a particularly elegant property and deserves to be highlighted.

- **Preserve mechanism/policy separation.** PromptForge should define what can be exposed and how execution behaves. The prompt author should decide whether a particular model is competent enough to receive a capability.

- **Add an "authority audit" debugging view if the runtime grows tooling around the language.** For each model turn, show the selected model, visible tools, whether orchestration capabilities were present, relevant context boundaries, and the source of injected data. This would make complex prompts much easier to inspect without changing the language itself.

- **Consider static diagnostics for unnecessarily broad tool scopes.** A linter could warn when a tool is declared or always exposed but never used, or when a section exposes many tools despite only one being referenced by the workflow. This would reinforce PromptForge's capability-minimization philosophy.

- **Consider static diagnostics for state that crosses a boundary accidentally through text rather than intentionally through the store.** Even a non-fatal warning could help authors recognize when a growing prompt is relying on fragile `reply` chaining where named persistent state would be clearer.

- **Provide one canonical sentence for PromptForge.** A strong candidate is:  
  **"PromptForge is a capability-scoped prompting language in which Markdown expresses model work, Lua orchestrates deterministic execution, and models may be granted additional authority only where the prompt author chooses."**

- **Protect the central invariant as the language evolves.** The model should never acquire authority merely because the runtime possesses it. Every meaningful transition of data, state, capability, context, or control should remain visible in the program.

