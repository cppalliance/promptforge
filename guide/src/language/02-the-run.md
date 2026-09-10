# The Run

You can now write a well-formed prompt file, so the next question is what happens when it runs. This chapter walks a run from beginning to end: the live pass over the title, the ordered walk through the sections, how results appear, and how a run finishes. Once you can picture a run, every other feature of the language has a place to attach.

## The preamble

When a run starts, the H1 section's Lua and prose blocks run first, in a live pass with full host access. This pass is the prompt's preamble. It is where the prompt declares which models and tools the run may use: `models.bind` and `models.default` declare model aliases resolved from capability descriptions, and `tools.bind` declares a tool alias the same way. Once the preamble finishes, those bindings are structurally frozen for the rest of the run.

A prompt with only an H1 title and no sections still runs. And a scalar `return` from the live H1 pass short-circuits the whole run: the returned value becomes the run's result, and no section ever fires.

Four calls are unavailable from the preamble: `call`, `jump`, `fanout`, and `list_from_section`. Each fails with "only available in sections". These calls move control between sections, so they exist only once the section walk has begun.

## The section walk

After the preamble, the top-level sections run in file order. The first H2 section in the file is the entry point, and control falls through from each section to the next.

Each section runs in its own isolated, sandboxed Lua state. Only the `string`, `table`, and `math` standard libraries plus safe base functions are available. The state is created at section entry and torn down at exit, so one section's Lua cannot leak into the next.

A section that talks to the model needs a model. `models.use` selects a bound alias for one section, and the prompt-wide default covers sections that select nothing; a model-facing call with neither fails with a model-required error. Tools follow the same pattern: `tools.always` or `tools.add` scope a bound tool to the model under its local alias.

## Lua blocks and prose blocks

The content of the H1 and of each section is an alternating sequence of `lua` fences and prose blocks. The classic shape is prologue, prose, epilog: a Lua block, then a prose block, then a Lua block.

Prose is data, not an implicit model turn. The prose written between a heading or Lua block and the next Lua block accumulates as a pending buffer, and that following Lua block reads it as the lazy `prose` value. Nothing is sent anywhere until Lua says so: `models.infer(prose)` runs one model round over it, and `messages.new()` plus `models.loop` builds a full conversation around it. The `{{ }}` placeholders in the buffer are substituted on the first read of `prose`, not before. Prose no Lua block ever reads is inert commentary, and prose left after a section's final Lua block is discarded when the section ends.

## Returns and the run result

A scalar `return` from a section's Lua block ends the run early with that value. When the first section returns `"first"`, a later section's own `return "unreached"` is never reached. A run in which no section returns finishes with the generic completion "done".

## What carries between sections

Sections are isolated in Lua, but three things roll forward through the walk. The `var` table is a per-run clipboard: it is seeded into each section's Lua state on entry and read back before teardown, so the next section sees the updates. The run-scoped `store` persists bulk state as virtual files addressed by logical string paths, shared across every section of the run. And everything else moves explicitly: `call(heading, input)` hands a subroutine its input and returns its result, so the author chooses what crosses a section boundary.

## Moving control between sections

Fall-through is only the default. A running section can also call `call(heading)` to run another section as a contained chain and get its return value back, `jump(heading)` to transfer control outright, and `fanout(worker, collection)` to run a worker section once per collection member concurrently. For now, hold the picture of the walk: preamble first, then sections in file order, with the clipboard, the store, and explicit call results rolling forward.
