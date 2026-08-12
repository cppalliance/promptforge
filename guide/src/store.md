# Store

The store is a run-scoped virtual filesystem shared across all sections. Data persists within a single run and the handle is thread-safe across concurrent tasks.

```lua
store.write("notes/summary.md", "# Summary\n" .. reply)
store.append("log.txt", "processed: " .. args .. "\n")

local content = store.read("notes/summary.md")
local numbered = store.read_lines("notes/summary.md")

store.str_replace("notes/summary.md", "old text", "new text")

local files = store.glob("notes/*.md")
local exists = store.exists("notes/summary.md")

store.delete("notes/summary.md")
```

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
