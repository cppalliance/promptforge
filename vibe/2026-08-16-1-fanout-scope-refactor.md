---
name: fanout-scope-refactor
overview: "fanout() accepts a Lua array as second arg + new items() function. Scope removal and models.only rename completed."
todos:
  - id: fanout-array
    content: fanout() accepts Lua array as second arg, dispatch on Value type in Lua binding
    status: pending
  - id: items-fn
    content: Add items(heading) Lua function that returns a section's pre-parsed bullet items
    status: pending
  - id: fanout-tests
    content: Tests for fanout with array and items()
    status: pending
  - id: fanout-docs
    content: Update guide/src/fanout.md and design-core.md item 20 with array syntax and items() documentation
    status: pending
isProject: false
---

## Plan

### Design principle

`model:infer(y)` in Lua, prose blocks, and `fanout(worker, items)` are all model interactions that read live tool/model state at their point in the walk. There is no scope open/close lifecycle - declarations are live until the VM is torn down. `models.only` in H1 forecloses `models.use` in H2 sections (and vice versa). Tool schemas are snapshot-read at each model interaction point.

### Completed (for reference)

- Scope lifecycle removed (d7ea4c0, 565873d, 0aee73e)
- models.always renamed to models.only with use foreclosure (995e33d)
- All tests fixed (9cb206f)
- Documentation sweep (463748a)
- extract_output cleanup (1c40dab)

### Remaining: fanout() accepts Lua array + new items() function

**Goal:** `fanout(worker, items)` where `items` is either a section heading string (current) or a Lua array of strings (new). New `items(heading)` function extracts pre-parsed bullet items from a list section.

**Files:**

- `promptforge/crates/promptforge-core/src/lua/vm.rs` (line ~1106): Change the `fanout` Lua function signature from `(String, String)` to `(String, Value)`. When `Value` is a string, resolve as section heading (current behavior). When it's a table, extract string items directly. Also add `items` as a new Lua global that takes a heading string and returns the section's pre-parsed items as a Lua table.

- `promptforge/crates/promptforge-core/src/execute/engine.rs` (`make_fanout_callback`): Change signature to accept `items: Vec<String>` directly instead of resolving from a list section. The callback in `run_section_lua` handles the string-vs-table dispatch before calling `make_fanout_callback`.

- `promptforge/crates/promptforge-core/src/fanout/mod.rs`: `run_fanout_arms` already takes `items: &[String]` - no change needed there. The `resolve_sibling` call for the list section moves from `make_fanout_callback` into the Lua-facing dispatch code.

**Documentation:**

- `promptforge/guide/src/fanout.md`: Add array-based fanout example alongside section-based example. Document `items()` function.
- `promptforge/crates/promptforge-core/design-core.md` item 20 (now item 21 after renumber): Update fanout description to mention array support and `items()`.

### Order of work

Steps, in order:

1. **Add fanout array support.** Change `fanout` Lua binding in vm.rs to accept `(String, Value)`. When `Value` is a table, extract strings directly. Update `make_fanout_callback` to accept `items: Vec<String>`. Add dispatch logic in `run_section_lua`.

2. **Add items() Lua function.** New global that takes a heading string, resolves the section, returns its pre-parsed items as a Lua table.

3. **Add tests.** fanout with Lua array, items() function.

4. **Update fanout docs.** guide/src/fanout.md, design-core.md item 21, guide/promptforge-user-guide.md fanout section. *(Verify: final step)*
