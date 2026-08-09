# `promptforge-dev`

Interactive runner for one PromptForge prompt against an already-running `promptforge-gateway`. This binary never starts the gateway or `llama-server`.

## Prerequisites

1. Start `promptforge-gateway` yourself (profile, models, and Brave credentials live there).
2. Export both:

```text
export PROMPTFORGE_GATEWAY_URL=http://127.0.0.1:8081/v1
export PROMPTFORGE_GATEWAY_KEY=<bearer from your gateway profile>
```

Missing either variable fails immediately with a short message telling you to start the gateway first. No prompt file is read until both are set.

## Usage

From the PromptForge repository root:

```text
cargo run -p promptforge-dev -- <prompt.md> [input] [--watch]
```

- `input` is optional and defaults to empty; it becomes the prompt's `args`.
- `--watch` re-reads, re-parses, re-binds, and re-executes after every save (300 ms debounce).
- There is no `--context`, `--max-tokens`, or `--no-think`. Declare those on the prompt under `models.need` / `models.always`.

## What happens on each run

1. Fetch the live model catalog from the gateway.
2. Prepare tools (`web_fetch` always; `web_search` when the same gateway credentials are present), the picker, and the model catalog for live H1 resolution.
3. Clear `<prompt-stem>.store/` beside the prompt, then execute.
4. During the run, store writes land under `<prompt-stem>.store/` immediately and each model turn writes `.trace/turn-N-*.json`. End-of-run reconcile removes orphans without wiping `.trace/`. Observer lines go to stderr; the result goes to stdout.

## Offline tests

```text
cargo test -p promptforge-dev
```

These cover arg parse, dump safety, watch debounce, tool registry construction, and missing-env failures. They do not contact a live gateway.
