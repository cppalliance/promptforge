Cooperative cancellation for the harness's async session paths: the awaitable [`CancelHandle`] a client selects over.

Dropping the outer future on Ctrl-C would abandon a run mid-step, so a host installs a [`CancelHandle`] with [`scope`] and calls [`CancelHandle::cancel`] from a Ctrl-C task instead, or selects over [`CancelHandle::cancelled`] beside the run's effect channel. [`maybe_scope`] installs a handle only when one is given, and [`current`], [`is_cancelled`], and [`wait_cancelled`] read the handle installed for the current task.
