# Control Flow

Fall-through runs sections in file order, but real prompts need to choose their path. This chapter teaches the two calls that move control, `jump` for transfer and `call` for subroutines, together with the visibility rules that decide which sections you may name. Learn the visible set first; everything else follows from it.

## The visible set

A running section can name as the target of `jump`, `call`, `fanout`, or `list_from_section` only its visible set: its sibling sections at the same heading level, excluding itself, plus its own direct child sections.

Heading references resolve on an exact level-and-name match. Zero matches is a not-found error listing only the visible set. Two matches is an ambiguity error rather than a silent pick.

## jump: transfer control

The call `jump(heading)` transfers control to a visible section, and the jumping section's remaining blocks never run:

````lua
jump('## Help')
store.write('seen.txt', 'should-not-run')  -- never runs
````

A jump carries the `var` clipboard across: the target's Lua state is seeded with the jumper's final `var`. Nothing else crosses implicitly, so pass state through `var` or the store.

A jump to a direct child heading starts a child-level walk over the jumper's children under the same rules, and the parent walk resumes after the jumper when the child level exhausts.

## call: a contained subroutine

The call `call(heading)` runs a visible section as a contained chain with a fresh Lua state, waits for it, and returns the chain's return value to the caller:

````lua
local summary = call('## Research')
````

The call clones the caller's `var` into the child chain and discards the child's writes when the chain ends, so a subroutine cannot disturb the caller's clipboard. An optional second parameter supplies an input string that overrides the run's `args` for the chain:

````lua
local summary = call('## Research', topic)
````

## Recursion and failure

Nested `call` and `fanout` recursion is capped at 8 levels, counting the first call. Exceeding the cap fails the call.

Suspending host calls such as `call`, `fanout`, and `models.infer` deliver failures as ordinary Lua errors. That means you can catch them with `pcall` and continue:

````lua
local ok, result = pcall(call, '## Research')
if not ok then
  log('research failed: ' .. tostring(result))
end
````
