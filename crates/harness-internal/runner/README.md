# Harness runner

This crate holds the per-run Harness and its effect loop. A Host builds one Harness per run and supplies its recorder, inference broker, timer, capabilities, and services, and the Harness then steps the Engine, performs each effect, returns each answer, and records the run. It owns no async runtime, so any async runtime can drive a run, and the Host can stop or cancel a run from outside. It is private to the Harness family, and clients reach it through the Harness's public API.
