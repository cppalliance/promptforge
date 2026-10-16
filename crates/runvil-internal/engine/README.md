# Engine executor

This crate is the Engine's executor: it runs a parsed prompt as a deterministic state machine. Every model round, tool call, file access, and timer leaves the executor as an effect for the Harness to perform and answer, and every boundary is reported as an event. Given the same prompt and the same answers, a run produces the same effects and events.
