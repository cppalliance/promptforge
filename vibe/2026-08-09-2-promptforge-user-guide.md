---
name: PromptForge user guide
overview: Write a progressive tutorial for PromptForge prompt authoring, starting from Hello World and building to a full-featured pipeline prompt, followed by a Lua API reference. Each section adds one capability with a working example.
todos:
  - id: s01
    content: "Section 1: What is a prompt file (skeleton + Hello World)"
    status: pending
  - id: s02
    content: "Section 2: The model turn (prose in, text out)"
    status: pending
  - id: s03
    content: "Section 3: Your first Lua block (the prologue)"
    status: pending
  - id: s04
    content: "Section 4: The preamble (H1 shared Lua)"
    status: pending
  - id: s05
    content: "Section 5: The epilog"
    status: pending
  - id: s06
    content: "Section 6: Multiple sections"
    status: pending
  - id: s07
    content: "Section 7: The store"
    status: pending
  - id: s08
    content: "Section 8: Template substitution"
    status: pending
  - id: s09
    content: "Section 9: Tools (search and fetch)"
    status: pending
  - id: s10
    content: "Section 10: The Tool object"
    status: pending
  - id: s11
    content: "Section 11: The Model object"
    status: pending
  - id: s12
    content: "Section 12: Explicit inference with model:infer()"
    status: pending
  - id: s13
    content: "Section 13: Alternating blocks"
    status: pending
  - id: s14
    content: "Section 14: Composable tool sets"
    status: pending
  - id: s15
    content: "Section 15: Sections as subroutines: execute()"
    status: pending
  - id: s16
    content: "Section 16: Control flow: goto()"
    status: pending
  - id: s17
    content: "Section 17: Fanout (parallel execution)"
    status: pending
  - id: s18
    content: "Section 18: The sys table"
    status: pending
  - id: s19
    content: "Section 19: Error handling and validation"
    status: pending
  - id: s20
    content: "Section 20: Capstone prompt (everything together)"
    status: pending
  - id: s21
    content: "Section 21: API Reference (all objects, functions, globals)"
    status: pending
isProject: false
---

# PromptForge User Guide

A progressive tutorial teaching prompt authoring from zero to full pipeline. Each section adds exactly one new concept with a working example. The guide ends with a capstone prompt using every feature and a complete API reference.

**Output:** `promptforge/user_guide.md` (single file, one read)

**Voice:** Direct, technical, no filler. Show the code first, explain after. Every example is a complete runnable prompt (or a section of one). No "in this section we will learn" - just the example and its explanation.

**Audience:** A developer who has PromptForge installed and a gateway running. They know markdown and have seen Lua. They have not read the design docs.

---

## Sections (one per step)

### 1. What is a prompt file

The skeleton: YAML frontmatter (`promptforge: 1`), one H1 title, one H2 section, prose text. The simplest possible prompt - no Lua, no tools, just prose that goes to the model and comes back.

### 2. The model turn

What happens when the executor runs that prose: it becomes a user message, the model responds, the response is the prompt's output. Show the mental model: prose in, text out.

### 3. Your first Lua block (the prologue)

Add a lua fence before the prose. Introduce `models.use`. Explain: the prologue runs before the model turn. Show a section with prologue + prose.

### 4. The preamble (H1 shared Lua)

Add a lua fence under the H1. Introduce `tools.need` and `models.always`. Explain: runs once, declarations available to every section. Show preamble + section.

### 5. The epilog

Add a lua fence after the prose. Introduce `reply` and `return`. Explain: epilog runs after the model finishes, same VM as prologue, can inspect and transform the result.

### 6. Multiple sections

Add a second H2. Show that sections execute in file order. Explain context clearing between sections: new VM, new conversation. The store and previous `reply` are the bridges.

### 7. The store

Introduce `store.write`, `store.read`, `store.inject`. Show one section writing, the next section reading. Explain: run-scoped virtual files shared across all sections.

### 8. Template substitution

Introduce `{{ args }}`, `{{ reply }}`, `{{ var.x }}`, `{{ sys.when }}`, `{{ item }}`. Show prose with substitutions. Explain: resolved before the model sees the text.

### 9. Tools (search and fetch)

Introduce `tools.need` in preamble, `tools.add` in prologue. Show a section that searches and fetches. Explain: the tool loop - model calls tools, gets results, keeps going until it produces text.

### 10. The Tool object

Show `tools.need` returning an object. Inspect `.name`, `.description`, `.parameters`. Pass it to `tools.add`. Build an array of tools. Override `.description` before adding.

### 11. The Model object

Show `models.always` returning an object. Inspect `.name`, `.model_id`, `.context`. Explain: the object represents a bound model you can use.

### 12. Explicit inference with model:infer()

Show `writer:infer(prompt)` in the prologue. Explain: blocks until the model responds, returns text, sets `reply`. Show the turn-gating pattern: add search, infer, add fetch, then prose.

### 13. Alternating blocks

Show a section with multiple lua/prose/lua/prose/lua blocks. Explain: non-final prose is single-shot (one round), final prose is the full tool loop. The conversation accumulates within the section.

### 14. Composable tool sets

Show building tool arrays in the preamble, storing in `var`, using conditionally based on `args`. Explain: tools are values you compose and pass around.

### 15. Sections as subroutines: execute()

Show `execute("## Research")` from Lua. Explain: fresh VM, full tool loop, returns reply. Show a pipeline orchestrated from a main section that calls other sections.

### 16. Control flow: goto()

Show `goto("## Fallback")`. Explain: context-clearing transfer, no return. The current section stops. Show a conditional branch based on store contents.

### 17. Fanout (parallel execution)

Show `fanout("## Worker", "## Topics")`. Explain: runs the worker section once per item in the topics list, in parallel. Returns an array of replies. Show the briefer evidence pattern.

### 18. The sys table

Document every field: `sys.when`, `sys.now`, `sys.id`, `sys.model`, `sys.taskid`. Show using `sys.when` in a report footer and `sys.model` for provenance.

### 19. Error handling and validation

Show epilog validation: checking `tools.calls`, asserting on `reply` content, returning errors. Show the "section incomplete" pattern. Explain: the epilog is your quality gate.

### 20. Capstone: a complete pipeline prompt

One full prompt that uses: preamble, tools, models, multiple sections, alternating blocks, store, execute, fanout, tool objects, model objects, infer, epilog validation. Annotated line by line.

### 21. API Reference

Every Lua global, object, and function in one flat reference. For each:

- Name and type (function / table / object / string)
- When available (preamble / prologue / epilog / always)
- Signature
- Return type
- Example

Organized by:
- Globals: `args`, `reply`, `item`, `var`, `sys`, `store`, `log`
- Objects: Tool (properties + methods), Model (properties + methods), Section/Task (properties)
- Functions: `tools.need`, `tools.add`, `models.always`, `models.need`, `models.use`, `execute`, `goto`, `fanout`
- Store methods: `write`, `append`, `read`, `read_lines`, `inject`, `str_replace`, `delete`, `glob`, `exists`

---

## Writing rules

- Every example is a fenced markdown prompt (or excerpt) that could be pasted into a `.md` file and run
- No forward references: each section uses only concepts introduced in prior sections
- Show the wrong way only when the right way is not obvious from the example
- Keep examples short (under 30 lines each, under 15 for simple concepts)
- Name the section's new concept in the H2 heading
- End each section with a one-sentence "what you now know" that names the capability
