# Workshop run log

This crate keeps Workshop's append-only record of every run: each effect, answer, and event the Harness records, in order, stored in a local database. Workshop, as the Host, supplies it to the Harness as each run's recorder, and nothing reads a record back into the Engine.
