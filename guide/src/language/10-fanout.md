# Fanout

Some work is the same task repeated over a collection: summarize each file, grade each answer, research each topic. The call `fanout` runs a worker section once per collection member, concurrently, and hands you one result per member. This chapter teaches the call, the shape of its results, and the isolation and failure rules that make concurrency safe. It closes the set because it uses everything before it: sections, the store, control flow, and limits.

## The basic pattern

The common shape pairs a list section with a worker section:

````lua
local replies = fanout("### Worker", list_from_section("### Topics"))
````

This runs the worker once per item of the list section. The second parameter must be a collection. The retired two-string form errors and points at `list_from_section`, and numbers and booleans error as not a collection. The worker must be a worker template section, not a list section; naming a list section is a Lua error. Fanout over an empty collection is an error raised before any scheduling, because no work is likely a bug.

## The collection

Fanout accepts any Lua table as its collection. The array part iterates in order first, then the hash part iterates in undefined order, with each hash member arriving as a pair table carrying `item.key` and `item.value`. Function members and table-keyed members cannot cross into an arm.

## Inside an arm

Each concurrent run of the worker is an arm. Inside an arm, the current collection member is available as the `item` global and as the `{{ item }}` substitution seed, and the arm's 1-based position is `sys.index`. Arms of a nested fanout restart numbering at 1.

## Results

Fanout results arrive in collection order, never finish order. Each result is a structured object with four fields: `.ok`, `.text`, `.item`, and `.exhausted`. Calling tostring on a result yields its text, so `table.concat(results, ',')` joins the texts directly.

An arm that produces no output inherits the reply incoming to the parent as its text. With no incoming reply it yields empty text and still reports ok.

## Isolation

Concurrency comes from interleaving chains at I/O points, not from worker threads, and at most the run's fanout window, 8 by default, run at once.

Each arm seeds `var` from a fresh clone of the caller's snapshot, so arm writes never cross arm boundaries or reach the caller. The store is shared, with one guard: two arms of one fanout writing the same store path fail with a write-write race error, while `store.append` to one path stays legal with unspecified order.

Arms can still rendezvous through the store. They write marker files and poll with `store.glob`, and each poll iteration yields through `execute` on a no-op section so sibling arms get scheduled.

## Failure semantics

A fatal arm error fails the fanout and aborts the sibling arms; the caller can catch it with `pcall` and continue. A softer case degrades instead: an arm whose tool loop exhausts becomes an incomplete stub result with `.ok == false` and `.exhausted == true`, and the sibling arms survive.

## Control flow from an arm

A fanout arm can jump. The arm's visible set is the fanout caller's visible set minus the worker, plus the worker's children. A child walk started from an arm runs with no item seed.

Recursion depth accumulates across a fanout boundary: an arm runs one execute level deeper than its caller, so an execute or fanout near the cap of 8 trips it.

## Workers on the shelf

An off-walk section still counts in `sys.section_count`, and it can serve as a fanout worker shared by multiple sibling callers. This is the natural home for a worker: written once, skipped by fall-through, and called by whoever needs it.

