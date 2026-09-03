# Tools

Models reach the outside world through tools, and a prompt controls exactly which tools the model can see. This chapter teaches the declaration and scoping calls, `tools.bind`, `tools.always`, and `tools.add`, plus local tools written in Lua, direct dispatch with `tool_call`, and the failure modes you will meet. Tool scoping is the prompt's main safety surface, so we build it up one call at a time.

## Declaring a tool

Declare a tool alias in the preamble with a natural-language capability description:

````lua
tools.bind('search', 'search the web')
````

The call `tools.bind` alone advertises nothing to the model; it only declares the alias. Binding resolves the description against the live catalog, and the failures are typed and specific: no match for the description, an ambiguous match listing the candidate identities, a duplicate alias, the same tool selected twice, or a picked tool absent from the live catalog. A capability description is resolved at most once per run, so repeated binds of the same description return the identical cached outcome, including identical failures.

## Scoping a tool to the model

Two calls scope a declared tool to the model under its local alias. The call `tools.always('search')` advertises the tool in every section. The call `tools.add('search')` advertises it in the current section only. To add several declared aliases at once, pass an array:

````lua
tools.add({"search", "fetch"})
````

The array form takes no per-element overrides.

You can replace the description the model sees. The call `tools.add(alias, override)` takes an override, and `tools.bind` and `tools.always` accept the same override as a trailing parameter. Precedence is the `tools.add` override over the `tools.bind` or `tools.always` override over the tool's catalog text.

The calls `tools.bind` and `tools.always` return a frozen Tool object with `name`, `description`, `parameters`, `wire_name`, and `untrusted` fields, and `tools.add` accepts Tool objects as well as alias strings.

## The tool loop

When a section's prose asks the model to use tools, the runtime loops: each tool call is dispatched and its result sent back as a tool message until the model replies with final text. Only the section's last prose block runs the full loop; earlier prose blocks run a single round. A `tools.add` called in a Lua block between prose blocks takes effect starting with the next prose block, not the one before it.

Calling `tools.add` with an alias that no `tools.bind` declared fails the run loudly, and the section prose never reaches a model. A model that calls a tool outside the section's advertised scope fails with an error listing the in-scope aliases, and the error notes when the alias was declared by `tools.bind` but not added to this section's scope.

## Local tools

You can write a tool in Lua with `tools['add_local']`:

````lua
tools['add_local']('grab', 'Grab a value', { value = 'string' }, function(args)
  return 'got ' .. args.value
end)
````

The handler runs as a Lua function in the section's own state. The parameter table is rendered to the model as a JSON schema with required properties. The handler's returned string goes back to the model verbatim and trusted. The handler can use `store` and section-global variables, but it cannot call `jump`, and a handler error fails the run with the handler's message. A local tool alias cannot collide with a `tools.bind` alias or with another local alias, and every tool schema advertised to the model is validated before it is sent.

## Direct dispatch and call counts

The call `tool_call(alias, args)` invokes any tool bound in the document directly from a Lua block, even one not scoped into the section, without widening the set advertised to the model. A `tool_call` with an alias that has no binding fails with an error listing every bound alias.

The counter `tools.calls[alias]` reads how many times the model has called a tool in the section. Reading it with an alias that was never bound is a hard error naming the bad key and listing the seeded aliases. The counter records a call even when the tool errors.

## Trusted and untrusted output

Output from a tool that marks its result untrusted is wrapped in a preface and nonce-tagged `<untrusted_input_...>` markers before the model sees it. Trusted tool output appends verbatim, and structured JSON output from a trusted tool resumes into Lua as a table.

## Validation and edge cases

Two semantic near-duplicate tools in one model-visible scope fail validation, with an error naming both aliases, both identities, and the similarity score. If you genuinely need both, isolate them in separate sections with per-section `tools.add`.

An empty final reply from the model fails the section unless a tool call preceded it and the finish reason is `stop`. A `length` finish reason returns the partial text and reports truncation. And a tool handler failure aborts the tool loop and fails the run with the tool's own error, preserving the underlying cause in the error chain.

