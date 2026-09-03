# Limits and Errors

Every run operates inside budgets, and every failure arrives in a stable shape. This chapter teaches the limits you can set, the defaults you get, and the error vocabulary you will see when something goes wrong. Knowing the failure shapes in advance is what makes a prompt debuggable.

## Capping the tool loop

The frontmatter key `max_tool_iterations` caps a section's tool-call loop:

````yaml
max_tool_iterations: 5
````

A model that keeps calling tools without converging stops after exactly that many round trips, and the run then fails with a tool-loop-exhausted error. The value must be a positive integer from 1 to 1000; zero, negative, and over-limit values are rejected at parse time.

## Default budgets

A run ships with these default limits:

- 24 tool iterations per section
- 8-way fanout concurrency
- a 16 MiB model response cap
- 64 MiB of Lua memory per section state
- 1024 Lua log events per section state
- a 120 second request timeout

A Lua block that exhausts a host resource quota fails with a typed quota error naming the exhausted resource: log events, log bytes, or instructions.

## The error kinds

A run failure is classified into one stable kind: parse, version, binding, completion, tool, store, lua, quota, substitution, cancelled, or internal. The kind tells you which layer rejected the run before you read the message.

Parse failures carry a stable classification kind and, when known, the location of the offending region. Lua compile errors name the prompt region and map back to the original source line numbers, so the error points at your file, not at generated code.

## Retrying and cancelling

Transient failures are marked retryable, so you can retry the run: transport errors, malformed responses, and backend failures with a 5xx status.

You can cancel a run with Ctrl-C. In-flight requests abort, and even an unbounded Lua loop stops, because an instruction-counting hook polls the cancel flag. The run ends with an "interrupted by Ctrl-C" error.

