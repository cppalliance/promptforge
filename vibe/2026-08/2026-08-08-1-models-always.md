---
name: models.always
overview: Add models.always(alias) in the H1 shared library so a prompt can set a default model binding for all sections without repeating models.use in every H2.
todos:
  - id: models-always-binding
    content: Add models.always(alias) to H1 binding mode with validation
    status: completed
  - id: models-always-replay
    content: Exact replay in SectionVm replay path
    status: completed
  - id: models-always-scope
    content: close_scopes falls back to always binding when no models.use
    status: completed
  - id: models-always-tests-docs
    content: Tests (binding, replay, execution, override, invalid) + docs; commit
    status: completed
isProject: false
---

# Add `models.always(alias)`

## What it does

`models.always("writer")` in the H1 shared library makes that model binding the prompt-wide default. A section that omits `models.use` gets the `always` binding instead of the host default. A section that calls `models.use("other")` overrides it for that section.

Parallel to `tools.always` - same H1-only constraint, same must-be-declared-first rule.

## Author experience

````markdown
# Briefer

```lua
models.need("writer", "A model for writing", { thinking = false, temperature = 0 })
models.always("writer")
tools.need("search", "Search the web.")
tools.always("search")
```

## Research

```lua
tools.add("search")
```

Search the web for {{ args }}
````

No `models.use` needed. Every section gets `writer` with `thinking = false, temperature = 0`.

Override when needed:

```lua
models.need("analyst", "A careful thinker", { thinking = true })
models.always("writer")  -- default
```

```lua
-- in one section
models.use("analyst")  -- override for this section only
```

## Implementation

### 1. Declaration recording

In [lua.rs](promptforge/crates/promptforge-core/src/lua.rs) / [lua_models.rs](promptforge/crates/promptforge-core/src/lua_models.rs) (wherever `models.need` binding lives):

- Add `models.always(alias)` callback in binding mode (same rules as `tools.always`: alias must have been declared by `models.need`, at most once, H1 only)
- Record it on `ModelBindings` (add an `always: Option<String>` field - only one model can be the default, unlike tools where multiple can be always-on)

### 2. Replay

In `SectionVm::new_with_shared_bindings` replay path: `models.always` must replay exactly like `tools.always` does. Same call-for-call contract.

### 3. Scope closure

In `close_scopes` / `close_model_scope`: when no `models.use` was called in the section, check if `ModelBindings::always` is set. If so, use that binding as the section's selected model. If not, keep `None` (host default).

Current behavior: `close_scopes` returns `ClosedScopes { model: None }` when no `models.use` was called. Change: return `ClosedScopes { model: Some(always_binding) }` when always is set and no explicit `models.use` was called.

### 4. Tests

- Binding: `models.need` + `models.always` records correctly
- Binding: `models.always` without prior `models.need` = error
- Binding: duplicate `models.always` = error (only one default)
- Replay: exact call-for-call
- Execution: section without `models.use` gets the always binding's `CompletionOptions`
- Execution: section with `models.use("other")` overrides the always
- Invalid: `models.always` called from H2 preamble = error

### 5. Docs

- README model selection section: document `models.always`
- design-core.md: update principle 10 ("omitting `models.use` keeps the host default client model" becomes "keeps the prompt-wide always binding, or the host default when none is declared")

## One commit

fmt, clippy `-D warnings`, full test pass. Single commit.
