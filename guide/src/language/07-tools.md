# Tools

Models reach the outside world through tools, and a prompt controls exactly which tools the model can see. Tools arrive in capabilities, the installation unit, and a prompt declares the capabilities it activates and the tool slots it binds in the frontmatter; the host fills every slot before the run begins. This chapter teaches the declaration, the advertising calls `tools.always` and `tools.add`, local tools written in Lua, direct dispatch with `tools.call`, and the failure modes you will meet. Tool scoping is the prompt's main safety surface, so we build it up one idea at a time.

## Capabilities and global names

A capability is the activation unit: code that runs at run setup and contributes tools. Every capability has a global id of exactly two segments, `namespace/pack`, where the namespace is a reverse-DNS name such as `io.github.corp` or the reserved first-party prefix `promptforge`. Every tool has a global path of exactly three segments, `namespace/pack/name`, and a tool's first two segments always name the capability that contributed it: `promptforge/web/fetch` comes from the `promptforge/web` capability, no exceptions.

Declare the capabilities a prompt activates with the `capabilities` key:

````yaml
capabilities:
  - promptforge/web
  - ref: io.github.corp/vault
    optional: true
````

A bare id declares a required capability: when it is absent from the host's registry or fails to activate, the run cannot start, and the preflight report names it. The map form with `optional: true` declares a capability the run skips with a log line when absent, so one prompt runs with or without an enhancement; the optional `config` key carries prompt-side data to the capability. User-specific configuration such as credentials is host-supplied and never named in the prompt.

## Declaring a tool slot

The `tools` key declares the run's tool slots, keyed by a prompt-local alias:

````yaml
tools:
  search:
    want: search the web
  fetch: promptforge/web/fetch
````

A bare string is an exact global path, filled by identity against the assembled catalog. Since the path's first two segments name its capability, a slot whose capability is not active cannot fill, and the preflight report says so. The map form is a fuzzy slot: the `want` prose is matched against the catalog at prepare by the picker, a local sentence-embedding model that maps English descriptions to tools, and every fill is journaled so you can see what the fuzz resolved to. A fuzzy slot with `optional: true` skips with a log line when nothing fills it.

## Binding versus advertising

Binding and advertising are separate facts. Binding is decided entirely at prepare: everything a binding decision could depend on - the frontmatter, the active capabilities, the assembled catalog - is known by then, and the journaled result is the run's bindings, alias to tool. What remains for run time is advertising: the prompt's Lua decides per section which already-bound aliases the model gets to see. The model only ever sees the alias, never the global path.

## Advertising a tool to the model

Two calls advertise a bound tool under its local alias. The call `tools.always('search')` advertises the tool in every section, conventionally from the H1 preamble. The call `tools.add('search')` advertises it in the current section only. To advertise several bound aliases at once, pass an array:

````lua
tools.add({"search", "fetch"})
````

The array form takes no per-element overrides.

You can replace the description the model sees. The call `tools.add(alias, override)` takes an override, and `tools.always` accepts the same override as a trailing parameter. Precedence is the `tools.add` override over the `tools.always` override over the tool's catalog text.

Each bound slot is also a bare global holding a frozen Tool object with `name`, `description`, `parameters`, `wire_name`, and `untrusted` fields, and `tools.add` accepts Tool objects as well as alias strings. Because `tools.always` records a prompt-wide fact in state every section shares, naming the same alias again is a no-op, so a shared library replayed into every section may name it.

## The tool loop

The tool loop lives inside `models.loop`. When the model answers a loop request with structured tool calls, the runtime dispatches each call to a tool in the section's scope, appends the correlated results to the message list, and asks again, until the model replies with terminal text. The scope is read at call time, so a `tools.add` earlier in the same Lua block applies to the `models.loop` call that follows it.

Calling `tools.add` with an alias that no frontmatter slot declared fails the run loudly. A model that calls a tool outside the section's advertised scope fails with an error listing the in-scope aliases, and the error notes when the alias was declared but not added to this section's scope.

## Local tools

You can write a tool in Lua with `tools['add_local']`:

````lua
tools['add_local']('grab', 'Grab a value', { value = 'string' }, function(args)
  return 'got ' .. args.value
end)
````

The handler runs as a Lua function in the section's own state. The parameter table is rendered to the model as a JSON schema with required properties; each value is a bare type string or a `{type, description}` pair. The handler's returned string goes back to the model verbatim and trusted. The handler can use `store` and section-global variables, but it cannot call `jump`, and a handler error fails the run with the handler's message. A local tool alias cannot collide with a declared slot alias or with another local alias, and every tool schema advertised to the model is validated before it is sent.

## The decision-tool recipe

When prose guidance should steer the run's shape - which sections to walk, which bound tools to advertise - do not ask the model for prose and string-parse the answer. Interpret the guidance into flags with a local decision tool in the H1 preamble:

````lua
tools['add_local']('decide', 'Record the verdict: one of use_mcp, no_mcp, or unspecified', { choice = 'string' }, function(args)
  var.verdict = args.choice
  return 'recorded'
end)
local msgs = messages.new()
msgs:user('Given these instructions, decide whether the private sources are needed: ' .. args)
models.loop(msgs)
````

The model's tool call lands in the Lua handler, which records the verdict where the walk can read it. Three rules keep the recipe honest. The choice set must include an explicit "unspecified" verdict, so a genuine abstention has a name. The no-call exit is handled in code: when the loop finishes without a call, `var.verdict` is simply unset, and the prompt treats that as abstention. And for weaker models that struggle with parameterized calls, the fallback is three no-arg tools, one per verdict, instead of one tool with a parameter. Decision-tool results are journaled like any tool call, so a replay consumes the recorded verdict rather than re-rolling it.

## Direct dispatch and call counts

The call `tools.call(alias, args)` invokes any tool bound in the document directly from a Lua block, even one not advertised in the section, without widening the set the model can see. A `tools.call` with an alias that has no binding fails with an error listing every bound alias. A Tool object works in place of the alias, so `tools.call(tool, args)` dispatches a held object directly.

The counter `tools.calls[alias]` reads how many times the model has called a tool in the section. Reading it with an alias that was never declared is a hard error naming the bad key and listing the declared aliases. The counter records a call even when the tool errors.

## Trusted and untrusted output

Output from a tool that marks its result untrusted is wrapped in a preface and nonce-tagged `<untrusted_input_...>` markers before the model sees it. Trusted tool output appends verbatim, and structured JSON output from a trusted tool resumes into Lua as a table. A bound tool's failure text is wrapped as untrusted whatever the tool's trust marking, so an error message from the outside world never reaches the model as trusted prose.

## Validation and edge cases

Two semantic near-duplicate tools in one model-visible scope fail validation, with an error naming both aliases, both identities, and the similarity score. If you genuinely need both, isolate them in separate sections with per-section `tools.add`.

An empty final reply from the model fails the loop unless a tool call preceded it and the finish reason is `stop`. A `length` finish reason returns the partial text and reports truncation. A bound tool's own failure does not abort the loop: the error message arrives as the call's tool result, wrapped as untrusted input, so the model reads the failure and the run continues. Cancellation and every other dispatch failure still abort the loop, and a local tool's handler error still fails the run.

## Migrating from tools.bind

Earlier versions bound tools from Lua, resolving a prose description against the catalog at run time. The declaration moved to the frontmatter, and binding moved to prepare. Before:

````lua
tools.bind('search', 'search the web')
tools.always('search')
````

After:

````yaml
capabilities:
  - promptforge/web
tools:
  search:
    want: search the web
````

````lua
tools.always('search')
````

The `tools.bind` call is removed. What was its prose description is now the fuzzy slot's `want`, filled by the picker at prepare with the fill journaled; an exact path fills by identity. The advertising calls, `tools.always` and `tools.add`, are unchanged.

## Designed, not yet built

Two extensions are designed but not yet built. The open posture, `tools: { open: true }`, lets a prompt accept whatever capabilities the host arms the run with instead of declaring its own; the `open` key is reserved today, so writing it fails the parse with a message saying so. And the prompt-pack capability contributes a directory of prompts as tools, one tool per prompt: invoking the tool runs the prompt as a sub-run, and the sub-run's result text becomes the tool output. Both arrive without structural change to what this chapter teaches.
