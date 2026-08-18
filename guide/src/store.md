# Store

The store is a run-scoped virtual filesystem shared across all sections. Data persists within a single run and the handle is thread-safe across concurrent tasks.

```lua
store.write("notes/summary.md", "# Summary\n" .. reply)
store.append("log.txt", "processed: " .. args .. "\n")

local content = store.read("notes/summary.md")
local slice = store.read("notes/summary.md", 20, 40) -- lines 20-40 only
local numbered = store.read_lines("notes/summary.md")

store.str_replace("notes/summary.md", "old text", "new text")

local files = store.glob("notes/*.md")
local exists = store.exists("notes/summary.md")

store.delete("notes/summary.md")
```

## Bounded Reads

`store.read` takes optional 1-based inclusive line bounds: `store.read("log.txt", 20, 40)` returns lines 20 through 40 joined by newlines, with no trailing newline. `store.read("log.txt", 20)` reads from line 20 to the end of the file. An `end` past the last line clamps to it, and a `start` past the last line returns an empty string. A `start` below 1 or an `end` before `start` raises an error.

## Safe Injection

`store.inject(path)` reads content wrapped in an untrusted-input guard envelope for safe re-injection into model prompts. Forged close-tags in stored content are escaped, so injected data cannot break out of the envelope:

```lua
store.write("user-data.txt", user_provided_content)
-- Later, safely inject into a prompt context:
local safe = store.inject("user-data.txt")
```

## Path Validation

All store paths are validated:

- Forward-slash only (backslash rejected)
- No path traversal (`.` and `..` segments rejected)
- No Windows reserved device names (CON, NUL, COM1-9, LPT1-9)
- No trailing dots or spaces
- Maximum 1024 bytes

## Glob Matching

- `*` matches within a single path segment
- `**` matches across path separators
- Unsupported syntax (backslash escapes, triple-star, misplaced `**`) is rejected
- Matching uses a bounded, non-backtracking algorithm

The `str_replace` operation requires the old text to be unique in the file; ambiguous matches are refused with a count of occurrences.

The default in-memory backend (`StoreRef::memory()`) requires no filesystem or network and drops cleanly with the run. Custom backends implement the `Store` trait.

## Pre-populated Stores

Callers can seed the store with files before a prompt runs. The MCP server does this when `input_file` or `input_text` is provided - it writes the content into the store at the path declared by the prompt's `input:` frontmatter before execution begins.

From the prompt's perspective nothing is different. It calls `store.read('paper.md')` and gets content. It does not know whether that content was placed there by an earlier section, by a caller, or by test scaffolding. This keeps prompts decoupled from their invocation context.
