# promptforge-cli

The `promptforge` command-line tool. It parses a PromptForge prompt file and executes its sections top to bottom (fall-through).

## Usage

```text
promptforge run <file.md> [input]
```

- `file.md` must be a PromptForge prompt: its frontmatter must declare a `promptforge:` version, or the CLI declines to run it.
- `input` is the single raw argument string exposed to the prompt as `args`; it defaults to empty.

Run `promptforge --help` or `promptforge run --help` for generated usage, and `promptforge --version` for the version.

## Gateway configuration

Gateway credentials are read only from the environment:

- `PROMPTFORGE_GATEWAY_URL` - the gateway base URL.
- `PROMPTFORGE_GATEWAY_KEY` - the bearer token.

With neither set, the CLI runs local-only (`web_fetch` only, no remote model catalog). Both are required together to reach the gateway and enable `web_search`; a key without a usable URL is rejected.

## Exit status

- `0` - success.
- `1` - an operational failure (unreadable file, not a prompt, parse, setup, or execution error).
- `2` - a usage error (owned by the argument parser).
- `130` - the run was cancelled with Ctrl-C.

## Publication

This is an unpublished leaf binary (`publish = false`, `version = "0.0.0"`).
