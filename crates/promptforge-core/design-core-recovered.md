# promptforge-core: a library that runs a markdown prompt file against a model

## Executive summary

`promptforge-core` turns a markdown file into a run. An author writes an ordinary markdown document with a small YAML header; the crate reads that file into inert data, then, on a separate call, executes it: it walks the document's top-level sections from top to bottom, runs a section's leading Lua block in a locked-down sandbox, substitutes `{{ }}` placeholders into the section's prose, sends that prose to a language model, lets the model call tools in a loop, and moves to the next section. A Lua block that returns a value ends the run; running off the last section resolves a final string.

The crate holds the parts a caller assembles into that run: the prompt parser, the section executor, a Lua sandbox, a `{{ }}` substituter, a chat-completions client for a gateway, a `Tool` trait plus one built-in web-search tool, a run-scoped virtual file store, and an observer seam that reports progress as it happens. A caller supplies the gateway client, the tool set, the store, and an observer; the crate supplies the control flow and the safety boundaries.

Two boundaries are load-bearing and deliberate. First, reading a prompt runs nothing: a server can discover files, ask "is this one of mine," and list what each prompt claims to do without executing a line of it. Second, a language model and any content a tool fetches are treated as untrusted: the Lua sandbox denies everything that reaches the host, a scoped tool a section never declared cannot be called, and output a tool marks as untrusted is fenced so a fetched page cannot pose as instructions to the model. A caller can rely on those two properties without reading the body of this document.

The engine runs one prompt format today (major version 1) and refuses anything else rather than guessing. The control flow it implements is a linear top-to-bottom walk; a reader extending the engine should treat that as the first shape built, not as a finished control-flow design, for reasons noted where the walk is described.

## Key design choices

1. **A prompt is a markdown file behind a small YAML header.** The file is a `---`-delimited YAML frontmatter block followed by a markdown body, and the body's heading structure is the section structure the engine runs. The body is markdown, and its prose is what the model reads as text. The cost is that the run's shape is tied to how a person happens to lay out headings.

2. **Reading a prompt never runs it.** Parsing produces plain data - a `Prompt` of sections, each holding its prose and its optional Lua source as raw strings - and nothing executes until a caller invokes the run entry point. A separate lenient probe answers "is this file a promptforge prompt, and for which engine major," returning an `Option` and never erroring, so a file walker can classify arbitrary files cheaply and a server can list prompts and show what each one claims to do without executing them.

3. **A file declares the engine major it targets, and the engine runs only what it supports.** Frontmatter carries two independent numbers: `version` is the prompt author's own contract, surfaced in the catalog listing, and `promptforge` is the engine major the file targets, checked at the run gate. The gate supports major 1 only: an unsupported major is refused with a distinct error, and a file that declares no engine version is declined as not one of ours. The refusal is deliberate and fails closed rather than best-effort running an unknown major as if it were 1.

4. **A run walks the top-level sections top to bottom, each in a fresh context.** Sections execute in file order; each gets a fresh system context whose id tracks its position, and nothing from one section's model reply flows into the next except through the shared store. There is no jump, branch, or successor construct. This makes file order load-bearing, and a reader extending the engine should not mistake this fall-through for a chosen control-flow design; it may be the only shape built so far.

5. **Each section computes in a sandboxed Lua fence before the model speaks.** A section's one leading ```` ```lua ```` fence is the only executable block; everything else is prose. The block runs first, and it runs in a VM that loads only the string, table, and math libraries and actively strips the code-loading and reflection globals. The block is untrusted, so everything that could reach the host is denied - the crate cannot escape the VM with unsafe code, so this boundary is where safety rests. A per-section instruction ceiling aborts a block that never terminates.

6. **A Lua return ends the run; running off the end resolves a result.** A section's Lua block that returns a value stops the whole walk, emitting the section-finished event before it does; this is the only in-Lua way to end a run early. If no block returns and control falls off the last section, the result is the author's declared `default_return` if present, else the last model reply, else the literal string `"done"`. Only a scalar Lua value becomes a result; a returned table stops the run with an error.

7. **Every started run ends with exactly one finished event.** A run that passes the version gate emits one started event, then one finished event carrying an `ok` flag, on both the success and the error path; a run refused at the gate emits neither. An observer can therefore pair a start with an end unconditionally, and can distinguish a run that failed mid-flight from one that never began.

8. **Tools are named trait objects, and a section narrows which the model may call.** Tools are `dyn Tool` trait objects dispatched by name, so a tool can live in a crate this one does not depend on and the set can be assembled at runtime from configuration. A section's Lua records tool names with `tools.add`; it validates nothing, because the sandbox has no registry to check against. The executor resolves those names against the run's pool, and a scoped name absent from the pool fails the whole run with an error naming the tool, rather than silently running the section unarmed.

9. **Untrusted tool output is fenced as data, not instructions.** A tool result is trusted and appended verbatim unless the tool opts out by declaring its output untrusted. Untrusted output is wrapped in a guard block whose delimiter carries an unguessable per-loop nonce, with any forged copy of the delimiter inside the content defanged, and a rule sentence telling the model the enclosed text is external data to analyze and not instructions to follow. This is a prompt-injection defense: a page a tool fetched cannot break out of the fence to command the model.

10. **The gateway client is supplied by the caller, never read from the environment.** The run takes its gateway client as an argument. This workspace forbids unsafe code, and on the 2024 edition setting an environment variable is unsafe, so a caller configured from a file could not place its gateway settings into the environment; handing in the client is the only way such a caller can supply one. An environment-based constructor remains as the fallback when the caller passes none, and the default model id is public so a file-configured caller reuses the identical fallback rather than spelling it a second time and drifting.

11. **Unresolved placeholders fail the run instead of rendering blank.** Prose substitution replaces `{{ }}` slots against the run's arguments and namespaces in a single non-recursive pass. A missing key, an unknown namespace, a value that is present but null, or an unclosed brace stops the run with an error naming the path, rather than emitting an empty string or the literal braces, so the model is never handed a prompt with a gap where a value should be. The single run argument is one raw string, addressable only as `{{ args }}`.

12. **Progress travels through an always-present observer with a fixed wire shape.** The run reports progress by calling an `Observer` it always holds - a no-op implementation stands in when a caller wants none, so the executor never branches on whether an observer is present. The event enum serializes externally tagged, and that JSON shape is pinned as a wire contract a consumer parses. The event set can grow, so an external consumer must keep a catch-all arm.

13. **A run shares one virtual file store across its sections.** The store is a cheaply-cloneable handle over a boxed backend behind a mutex, required because a synchronous Lua VM and asynchronous model tools both hold the same store across await points, so it must be shareable across threads. Reads return numbered lines and edits are made by a substring anchor that must match exactly once, or the edit is refused with the match count.

14. **One error type covers parsing, transport, and execution.** A single crate-wide `Error` enum spans the whole pipeline: one result type threads from parse through client to execution. The transport variant hides its underlying source behind a boxed error so the transport library never appears in the public surface and can be swapped without a breaking change. The public data and error types are marked so new fields and variants can be added later without breaking downstream crates.

## The prompt file is markdown a person writes and a model reads

A prompt file is a YAML frontmatter block, delimited by a leading and closing `---`, followed by a markdown body. The frontmatter is typed and deserialized; the body is parsed as markdown, and its heading hierarchy becomes the run's sections. The prose inside a section is kept as a raw string, because that prose is what the model reads.

A minimal file:

````
---
name: greet
description: Greets a person by name
version: 1
promptforge: 1
---

# Greet

```lua
tools.add("web_search")
```

Say hello to {{ args }}, and look up anything you need.
````

Three frontmatter fields are required and the rest carry defaults:

- Required: `name`, `description`, `version`. These are read by every consumer regardless of what a prompt does - the name a caller passes to run it, the description a catalog shows, the author's contract version - so a file lacking any of them cannot be used.
- Optional: `promptforge` (the engine major, absent meaning "not one of ours"), `tools`, `default_return`, and `max_tool_iterations`. Each has a meaningful absence, so each defaults rather than being demanded.

`version` and `promptforge` are two different numbers on purpose: one versions the author's prompt, the other selects the engine that runs it. They are read by different code for different ends and neither is an alias of the other.

## Parsing turns the file into inert data

Parsing produces a `Prompt`: a list of top-level sections, each carrying its name, its level, its raw prose, and at most one block of Lua source. A section's executable region is a single ```` ```lua ````-tagged fence, and only if it is the first thing in the section; a fence that is not first, or not tagged `lua`, or never closed, stays in the prose. There is at most one Lua block per section.

Sections also form a recursive tree by heading depth, so an `## H2` owns the `### H3` sections beneath it. The tree records the author's outline. The executor, though, walks only the flat top-level list and never descends into a section's children, so sub-sections nested under a heading are parsed but never run; an author cannot yet rely on nesting to scope behavior. A public accessor names the first top-level section as the prompt's entry point, chosen by position rather than by any reserved name, though the executor starts from the head of the list directly rather than through that accessor.

## A run gates on the engine major before it emits any event

The run entry point takes the prompt, the single argument string, the tool pool, and the store as positional inputs, and bundles the two environment inputs - the observer and an optional gateway client - into an options struct, so the call keeps a short arity and the environment set can grow without breaking callers. The client is optional: when a caller passes none, the run builds one from the environment.

Before any work, the run gates on the declared engine major. A supported major proceeds; an unsupported one is refused; a missing one is declined as not a promptforge prompt. This check runs before the started event, which is why a gate-refused run reports nothing at all. Past the gate, the run emits its started event, walks the sections, and emits exactly one finished event carrying whether it succeeded.

The environment-based client constructor requires only the gateway's shared bearer token and defaults the base URL and the model, because the token is the one setting with no safe default while a local gateway URL and a model id both have one.

## A section runs its Lua fence, then loops its substituted prose against the model and its scoped tools

Each section runs in a fresh context whose id is its position in the walk. If the section has a Lua block, it runs first, in the sandbox described above. The block can read the run's inputs and record the tools this section allows; it does not itself call the model.

Inside the block, a small host surface is bound: the raw `args` string, the run's variables and system context, and a `store` table. The store table is always present, placed alongside the other host state rather than among the per-section scoped tools, because it is a host capability the runtime provides rather than a model-facing tool. Tools, by contrast, are scoped per section: `tools.add` records names into a de-duplicated, first-seen list for the executor to resolve, and validates nothing itself. A block that returns a scalar ends the whole run with that value; a block that returns a table is an error.

After the block, the section's prose has its `{{ }}` placeholders substituted in one pass and is sent to the model. The model may answer with text or with a request to call tools; a tool-call answer runs the tools and loops. The loop is bounded by the section's `max_tool_iterations`, or a default cap of 24 when the frontmatter sets none. When the model answers with text, the section is done and the walk continues, unless a Lua return already ended it.

The executor walks the sections in file order, each in its fresh context, and the only thing that crosses from one section to the next is the shared store; a section's model reply does not flow into the next section's prose. Nothing in the crate jumps, branches, or selects a successor - a reader extending the engine should not read this file-order fall-through as a settled control-flow model rather than the first shape built.

## A model turn returns either text or tool calls, and tools are trait objects dispatched by name

A single chat-completion round trip returns one of two outcomes: a final text message, or a request for tool calls - never a stream. The executor's loop matches on the two: text ends the turn, tool calls continue it. Each outgoing message serializes only the fields its role uses, so an ordinary user message carries just its role and content. When the model requests a tool call whose arguments are not valid JSON, the client preserves the raw arguments as a string value rather than aborting, leaving interpretation to the tool.

Tools are `dyn Tool` trait objects, so a tool implemented in a crate this one does not depend on can still be registered, and a tool set can be built from runtime configuration into one heterogeneous collection - neither of which a closed enum or a compile-time generic could hold. A tool declares whether its output is untrusted; the default is trusted, and only a tool returning attacker-shaped external content overrides it. Trusted output is appended verbatim; untrusted output is fenced in the nonce-delimited guard block so the model reads it as data.

The one built-in tool, web search, posts to the gateway rather than to a search provider directly, so the vendor's search key never enters the process running the prompt - that process holds only the gateway's shared token. It returns the gateway's JSON body verbatim as a string; the crate imports none of the gateway's result types, so it stays free of that schema. The guard-block nonce is drawn from a fast non-cryptographic source, minted per tool-loop invocation and never reused across runs. On a failed request, the error body is truncated to 2000 characters.

## One shared file store is a cloneable handle over a locked backend

A run holds one virtual file store, shared across its sections through a cloneable handle over a mutex-guarded boxed backend. Sharing is by clone rather than by mutable borrow: every operation takes a shared reference and locks internally. The handle is required to cross threads because a synchronous Lua VM and asynchronous tools both use it; the boxed backend behind a trait is what lets a future filesystem or network backend replace the in-memory one, and its operations already return results even though the in-memory backend never fails.

Reads return the file's contents as numbered lines - one-based numbers right-aligned to a common width, an empty file reading as empty rather than a bare number. Edits are made by a substring anchor: the anchor must match exactly once, and an edit is refused with a match count if the anchor is missing or ambiguous, rather than applied to an arbitrary match. Listing supports `*` within a path segment and `**` across separators, matching the globbing an author already knows. When the store's lock is found poisoned by a panic elsewhere, the operation recovers the guard and continues rather than propagating the poison.

## One error type spans the pipeline, and the public types are marked for additive change

One `Error` enum spans parsing, transport, Lua, substitution, and execution, so a single result type threads through the whole pipeline; its variants carry specific payloads and distinct messages - a backend status and body, the name of an unknown scoped tool, an unsupported version number. The transport variant boxes its source so the transport library is not named in any public signature, which is what lets that library be swapped without breaking callers.

The public data types - the frontmatter, section, and prompt structures, the wire message and tool types, the event enum, the store error, and the crate error - are marked so that new fields and variants can be added in a later minor release without breaking a downstream crate that matches or constructs them. That mark sits on exactly the caller-facing types and is absent from the private ones and the service structs, so a downstream consumer must construct through the provided constructors and keep a catch-all match arm.

## A run always reports progress to an observer, over a pinned wire shape

A run reports progress by calling an observer it always holds. A caller that wants no reporting passes a no-op observer, so the executor never checks whether one is present; a real reporting observer lives in the server that hosts runs. The events are a started event carrying the section count, a per-section started event, a per-section finished event, and a run-finished event carrying success. The section-started event carries a count of sections entered so far; that count rises by one per section and so never goes backward today, but it does so only because the walk is linear, and a consumer that needs a strictly monotonic display should not assume the core guarantees it under a future non-linear walk - a downstream consumer re-clamps it rather than trusting it.

The event enum serializes externally tagged and that shape is pinned as a wire contract:

```json
{ "RunStarted": { "sections": 3 } }
{ "RunFinished": { "ok": false } }
```

A consumer dispatches on the variant name in that outer position, and because the event set can grow it keeps a catch-all for variants added later.

*2026-08-04 - Claude Opus 4.8*
