# promptforge-cli

[![Crates.io](https://img.shields.io/crates/v/promptforge-cli.svg)](https://crates.io/crates/promptforge-cli)
[![docs.rs](https://img.shields.io/docsrs/promptforge-cli)](https://docs.rs/promptforge-cli)
[![License](https://img.shields.io/crates/l/promptforge-cli)](LICENSE)

A command-line tool that runs PromptForge prompt files in a single process. Point it at a prompt file, and it parses the sections, executes them top to bottom, and prints the returned value. No server to start, no connection to manage, no configuration to write. You edit a prompt, run it, and see what it produces.

## Installation

```bash
cargo install promptforge-cli
```

## Usage

```bash
promptforge run prompts/hello.md
promptforge run prompts/staker.md "Bloomberg"
```

The prompt's returned value goes to stdout. Errors go to stderr. On success, stdout contains exactly the returned value and nothing else.

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
