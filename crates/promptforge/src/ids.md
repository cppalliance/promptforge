The identities of a run's chains and tasks, and the provenance on every effect and event.

# Chains and tasks

Every chain a run executes - the main walk, a `call` child, a spawned task - is named by a [`ChainId`]: its parent chain's id extended by the parent's local child counter, which `call` children and spawned tasks share. The main walk is the root chain `0` ([`ChainId::root`]). A task's [`TaskId`] is its chain's id; the separate type keeps a task-keyed table from accepting an arbitrary chain by accident. A section entry's id, which Lua reads as `sys.id`, is its chain's id extended by the chain's local entry counter ([`ChainId::entry`]).

Two runs of the same prompt with the same inputs allocate the same ids however their chains interleave, because every counter is local to the chain that advances it. An id renders and parses as a dot-separated path of decimal components (`0`, `0.2`, `0.2.0`), which reads as the hierarchy it names in a log or a UI; text that is not such a path fails with [`ParseIdError`]. Ids order as paths: a chain before its descendants, siblings by index, so sorting the tasks one chain owns recovers their spawn order.

# Provenance

A [`Provenance`] is the replay key stamped on every effect and event: the nearest enclosing task and the item's sequence number within it. The main walk reports under task `0`, and a `call` child reports under its parent's task, which is unambiguous because a call blocks its parent. The sequence counter is local to the task and shared by its effects and its events, so the two kinds order against each other within one task. A log slices by task and orders within a task by provenance alone, and a replay matches a re-executed run against its record by it.

```
use promptforge::ids::{Provenance, TaskId};

let task: TaskId = "0.2".parse()?;
let first = Provenance { task: task.clone(), seq: 0 };
let second = Provenance { task, seq: 1 };
assert!(first < second);
assert_eq!(first.task.to_string(), "0.2");
# Ok::<(), promptforge::ids::ParseIdError>(())
```

# Task origins and ends

A [`TaskOrigin`] names the principal that started a task: the prompt's author through `tasks.spawn`, or the model through its `task` tool. The two end differently when their owner ends first: an author task that outlives its owner is the author's bug and fails the chain, while a model task is abandoned and reported. An [`AbandonReason`] says which kind of owner end it was, so the log and the model notice can say more than "abandoned".
