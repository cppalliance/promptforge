The run log's error vocabulary: the [`LogError`] a failed run log operation returns and the [`RunId`] a session's runs are known by.

Every session run is recorded in the harness run log, so a client meets it in three places: [`crate::LaunchError::Log`] when the log cannot be opened at launch, [`crate::Session::transcript`] when a run cannot be read back, and [`crate::Session::run_ids`], the session's runs in launch order. A [`RunId`] is meaningful only against the log that issued it.

[`LogError`]'s database and payload variants carry their cause as a [`DatabaseSource`] or a [`JsonSource`], so the error surface names no database engine or JSON type. Each wrapper renders and sources exactly as the error it wraps, and its `as_inner` and `into_inner` restore the wrapped error for a client that branches on it.
