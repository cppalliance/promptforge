---
name: Store input files support
overview: Add support for pre-seeding the MemStore with input files before a prompt run, and for declaring input/output files in prompt frontmatter, enabling sandboxed file-in/file-out prompt execution without disk IO.
todos:
  - id: memstore-with-files
    content: Add MemStore::with_files and StoreRef::with_files constructors
    status: completed
  - id: frontmatter-files
    content: Add optional top-level input and output fields to Frontmatter struct
    status: completed
  - id: mcp-schema
    content: Add input_file, input_text, output_file string parameters to run_prompt schema
    status: completed
  - id: runner-seed
    content: Read input_file from disk or use input_text, write to store at declared path before run
    status: completed
  - id: runner-extract
    content: After run, read declared output from store; write to output_file or return inline
    status: completed
  - id: tests
    content: Add tests for store seeding, frontmatter parsing, validation rules, and round-trip file IO
    status: completed
  - id: user-guide
    content: Update promptforge user guide with input/output documentation and examples
    status: completed
  - id: design-doc
    content: Update design-mcp-server.md and design-core.md with input/output file design
    status: completed
isProject: false
---

# Store Input Files Support

## Current Architecture

The run pipeline is:

1. MCP `run_prompt` receives `{ prompt: "name", args: "raw string" }`
2. Runner creates a fresh empty `StoreRef::memory()` ([runner.rs:273](promptforge/crates/promptforge-mcp-server/src/server/runner.rs))
3. `execute::run()` takes `store: &StoreRef` and threads it through all sections
4. Lua code accesses files via `store.write()`, `store.read()`, etc.
5. Run completes, returns a string; the store is dropped (contents lost)

The store is already a trait (`dyn Store + Send`) behind `StoreRef` - it can be pre-populated before the run starts. The `MemStore` uses a `BTreeMap<String, String>` internally.

## Design

### 1. Frontmatter: declare `input` and `output`

Add optional top-level `input` and `output` keys to `Frontmatter`. Single-file form (one input, one output):

```yaml
---
name: papergate
description: Analyze a WG21 paper and produce a gating report
promptforge: 1
input:
  path: paper.md
  description: The paper markdown to analyze
output:
  path: report.md
  description: The gating report produced by analysis
---
```

- `input` - a file the prompt expects to find in the store when it starts
- `output` - a file the prompt will leave in the store when it finishes (contractual)
- `path` - the store-internal filename (what Lua uses: `store.read('paper.md')`)
- `description` - documentation, also feeds MCP schema generation

Both are optional. The prompt doesn't know or care whether the content came from a file on disk or inline text.

**Visibility:** `Frontmatter` fields are `pub(crate)` but it already exposes public getters (`name()`, `description()`, etc.). The MCP server accesses them via `prompt.frontmatter().name()`. Follow the same pattern - add accessors on `Frontmatter`:

```rust
impl Frontmatter {
    pub fn input(&self) -> Option<&FileDecl> { self.input.as_ref() }
    pub fn output(&self) -> Option<&FileDecl> { self.output.as_ref() }
}
```

### 2. MCP schema: flat string parameters

The `run_prompt` tool schema adds three optional string parameters:

```json
{
  "name": "run_prompt",
  "inputSchema": {
    "type": "object",
    "properties": {
      "prompt":      { "type": "string", "description": "The prompt to run." },
      "args":        { "type": "string", "description": "Textual input." },
      "input_file":  { "type": "string", "description": "Path to read into the store." },
      "input_text":  { "type": "string", "description": "Text to place in the store." },
      "output_file": { "type": "string", "description": "Path to write the output to. Omit to return inline." }
    },
    "required": ["prompt"]
  }
}
```

All five parameters are flat strings. Any model can fill this in.

**Schema descriptions should reference the prompt's declaration.** The `list_prompts` result should include `input` and `output` metadata (path and description) for each prompt so the calling model knows what file to provide and what output to expect.

**Validation rules:**
- If both `input_file` and `input_text` are present, reject with `-32602` ("specify one, not both")
- If `input_file` or `input_text` is provided but the prompt has no `input` declaration, reject
- If the prompt declares `input` but neither `input_file` nor `input_text` is provided, reject

**Example calls:**

File input, file output:
```json
{ "prompt": "papergate", "args": "Focus on ABI", "input_file": "c:/papers/p2996r7.md", "output_file": "c:/output/report.md" }
```

Text input, inline output:
```json
{ "prompt": "papergate", "args": "Focus on ABI", "input_text": "# P2996R7\nThis paper..." }
```

No input (prompt that only uses args):
```json
{ "prompt": "summarize", "args": "Explain monadic error handling" }
```

### 3. Gateway execution flow

```
Caller: run_prompt({ prompt, args, input_file|input_text, output_file })
  |
  v
Gateway resolves input:
  input_file -> read from disk
  input_text -> use value directly
  |
  v
Gateway seeds MemStore at the frontmatter's input.path
  |
  v
execute::run(prompt, args, ..., &store) -- prompt is sandboxed
  |
  v
Gateway reads the frontmatter's output.path from the store
  |
  v
output_file present -> write to disk, return path in result
output_file absent  -> return content inline in result
```

- If `input_file` points to an unreadable path, reject before the run starts
- If the prompt didn't produce its declared output, report in result without failing the run
- The prompt only sees the MemStore - never the real filesystem

**Safety model:** The orchestrator owns both source and destination paths. The gateway performs no path authorization - it trusts the caller has filesystem access. The prompt is fully sandboxed in MemStore.

### 4. MemStore: add `with_files` constructor

Add a convenience method to `MemStore`. Each path is validated through `StorePath::parse` at construction time so the store never holds a path that `store.read()` can't reach:

```rust
impl MemStore {
    pub fn with_files(files: impl IntoIterator<Item = (String, String)>) -> Result<MemStore, StoreError> {
        let mut map = BTreeMap::new();
        for (path, contents) in files {
            let validated = StorePath::parse(&path)?;
            map.insert(validated.as_str().to_owned(), contents);
        }
        Ok(MemStore { files: map })
    }
}
```

And on `StoreRef`:

```rust
impl StoreRef {
    pub fn with_files(files: impl IntoIterator<Item = (String, String)>) -> Result<StoreRef, StoreError> {
        Ok(StoreRef::new(Box::new(MemStore::with_files(files)?)))
    }
}
```

## Key Files to Modify

**Core:**
- `promptforge/crates/promptforge-core/src/parser/build.rs` - extend `Frontmatter` with `input`/`output` fields
- `promptforge/crates/promptforge-core/src/store/mem.rs` - add `MemStore::with_files`
- `promptforge/crates/promptforge-core/src/store.rs` - add `StoreRef::with_files`

**MCP server:**
- `promptforge/crates/promptforge-mcp-server/src/tools.rs` - add `input_file`, `input_text`, `output_file` to schema
- `promptforge/crates/promptforge-mcp-server/src/server/runner.rs` - seed store, extract output, write to disk
- `promptforge/crates/promptforge-mcp-server/src/server.rs` - validate mutual exclusion, pass new params to runner
- `promptforge/crates/promptforge-mcp-server/src/server/listing.rs` - include input/output metadata in `list_prompts` result
- `promptforge/crates/promptforge-mcp-server/src/result.rs` - extend `RunResult` to carry output content/path

**Tests:**
- `promptforge/crates/promptforge-core/src/store/tests.rs` - `with_files` constructor tests
- `promptforge/crates/promptforge-core/src/parser/build.rs` - frontmatter parsing tests for `input`/`output`
- `promptforge/crates/promptforge-mcp-server/src/server/tests.rs` - round-trip integration tests

**Design docs (update existing, do not create new files):**
- `promptforge/crates/promptforge-core/design-core.md` - document store seeding from caller, input/output frontmatter
- `promptforge/crates/promptforge-mcp-server/design-mcp-server.md` - document input_file/input_text/output_file handling, validation rules

**User guide:**
- `promptforge/guide/src/store.md` - document pre-populated stores
- `promptforge/guide/src/prompt-files.md` - document `input:`/`output:` frontmatter
- `promptforge/guide/src/mcp-server.md` - document `input_file`/`input_text`/`output_file` params
- `promptforge/guide/src/getting-started.md` - add a file IO example

## Execution References

Load these before building:
- `c:\Users\Vinnie\src\cursor\tools-public\rulebooks\rust-rulebook.md` - Rust coding standards (formatting, errors, API design, testing, docs)
- `c:\Users\Vinnie\src\cursor\tools-public\rulebooks\vibe-rulebook.md` - Work loop: plan into testable commits, coder/review-and-fix/verify subagents
- `c:\Users\Vinnie\src\cursor\tools-public\tools\architect.md` - Design doc updates (integrate into existing per-crate docs, not a new file)

## Design Decisions (settled)

- Frontmatter uses `deny_unknown_fields`, so adding `input`/`output` requires the serde model change
- All MCP parameters are flat strings - no nested objects, no maps
- `input_file` and `input_text` are mutually exclusive; both present is `-32602`
- `output_file` absent means return content inline; present means write to disk
- Output extraction is best-effort: missing output is reported, not a run failure
- The gateway does no path authorization - the orchestrator is trusted
- Single input/output only for now; multi-file is a future extension
