# workshop-run-log

The Workshop run log: an append-only Turso record of every run and, per run, every effect, answer, and event in loop order. `TursoRecorder` implements the Harness's `RunRecorder` over it, so Workshop, as the Host, keeps each run the Harness records in `runs.db` under the server's state directory. The database opens on the first run and a failed open is tried again by the next call. Nothing reads a record back into the Engine, and the Harness holds no database of its own. A feature-tier crate in `crates/workshop/`: it names the Harness only through `harness::record`.
