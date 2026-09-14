---
name: Gateway dotenv support
overview: Add `dotenvy` to the gateway so it walks the full config include chain, loading every name-matched env file (e.g. `gateway.env` for `gateway.toml`) with root-wins precedence, before interpolation resolves `${VAR}` references.
todos:
  - id: workspace-dep
    content: Add dotenvy to workspace Cargo.toml dependencies
    status: completed
  - id: crate-dep
    content: Add dotenvy.workspace = true to gateway Cargo.toml
    status: completed
  - id: load-env
    content: Walk include chain for first matching .env file in profile.rs load_path()
    status: completed
  - id: gitignore
    content: Add *.env to .gitignore
    status: completed
  - id: factor
    content: Factor env-chain walk into collect_config_chain + load_env_chain for testability
    status: completed
  - id: tests
    content: "Write tests: chain ordering, layered precedence, no env file, partial chain, process env precedence"
    status: completed
  - id: verify-dep
    content: Verify dotenvy on crates.io is the intended package before adding it
    status: completed
  - id: build-test
    content: Build, fmt, clippy, and run full test suite
    status: completed
isProject: false
---

# Add config-matched `.env` file loading to the gateway

## Why

The gateway's `${VAR}` interpolation in `config/interpolate.rs` calls `std::env::var()`. When the process environment is missing a variable (Cursor agent shells, minimal service environments), the load fails. A secrets file next to the config provides secrets without depending on shell inheritance.

## Design: name-matched env files

The env file name mirrors the config file name with a `.env` extension:

- `gateway.toml` - `gateway.env`
- `profiles/dev.toml` - `profiles/dev.env`
- `profiles/prod.toml` - `profiles/prod.env`

This avoids ambiguity when multiple configs share a directory - each gets its own secrets file. Missing env files are silently ignored (production uses real env vars).

## What changes

### 0. Verify the `dotenvy` crate

Before adding the dependency, confirm `dotenvy` on crates.io is the intended package (maintained fork of `dotenv`, by Allan Zhang / the `dotenvy` maintainers). Check: last release date, license (MIT), repository URL, `unsafe` count, dependency tree size. Per the Rust rulebook: "a hallucinated or near-miss name compiles like any other."

### 1. Add `dotenvy` to the workspace

In [Cargo.toml](Cargo.toml) (workspace root), add to `[workspace.dependencies]`:

```toml
dotenvy = "0.15"
```

### 2. Add `dotenvy` dependency to the gateway crate

In [crates/promptforge-gateway/Cargo.toml](crates/promptforge-gateway/Cargo.toml), add under `[dependencies]`:

```toml
dotenvy.workspace = true
```

### 3. Walk the include chain for the first env file

The env file is found by walking the config include tree (root first, includes after) and loading the first `*.env` match. This happens inside `profile.rs`, between include-tree resolution and `${VAR}` interpolation.

**a) Collect config paths during the include walk**

In [crates/promptforge-gateway/src/profile.rs](crates/promptforge-gateway/src/profile.rs), add a `config_chain: &mut Vec<PathBuf>` parameter to `load_value()`. Push each config path at the top of the function:

```rust
fn load_value(
    path: &Path,
    depth: usize,
    stack: &mut Vec<PathBuf>,
    visiting: &mut HashSet<PathBuf>,
    config_chain: &mut Vec<PathBuf>,  // NEW
) -> Result<Value, ConfigError> {
    // ...
    config_chain.push(path.to_path_buf());  // NEW - collects depth-first order
    // ... rest unchanged, just pass config_chain through recursive calls
}
```

**b) Search for env file after the walk, before interpolation**

In `load_path()`, iterate the collected paths and load the first matching env file:

```rust
pub(crate) fn load_path(path: &Path) -> Result<Config, ConfigError> {
    let mut stack = Vec::new();
    let mut visiting = HashSet::new();
    let mut config_chain = Vec::new();
    let value = load_value(path, 0, &mut stack, &mut visiting, &mut config_chain)?;

    for config_path in &config_chain {
        let env_path = config_path.with_extension("env");
        if env_path.exists() {
            let _ = dotenvy::from_path(&env_path);
        }
    }

    Config::from_value(value)  // interpolation now sees the loaded env vars
}
```

**Layered example:**

```
promptforge-gateway serve gateway.toml

gateway.toml includes ["shared/base.toml"]
  shared/base.toml includes ["shared/common.toml"]

Walk: gateway.env → shared/base.env → shared/common.env
All found env files are loaded. Root values take precedence (dotenvy
does not override already-set vars), parents supply defaults.
```

Key behaviors:
- Root config loaded first (most specific), then includes in depth-first order
- `with_extension("env")` replaces `.toml` with `.env`
- All matching env files are loaded, not just the first
- `dotenvy::from_path()` does not override existing vars, so root values win
- No env files found = silently continues, falls back to process environment
- `load_named()` calls `load_path()`, so profiles inherit the same behavior

### 4. Gitignore `*.env`

Add `*.env` to [.gitignore](.gitignore) - covers `gateway.env`, `dev.env`, any profile env files.

### 5. Factor env-chain logic for testability

Split the env-file walk into two `pub(crate)` functions in `profile.rs`, documented per the Rust rulebook (doc comment on every non-private item, `# Errors` on anything returning `Result`):

```rust
/// Collects config file paths in include-chain order (root first, depth-first)
/// alongside the merged TOML value.
///
/// Testable without touching the process environment.
///
/// # Errors
/// Returns [`ConfigError`] on read, include, parse, or validation failure.
pub(crate) fn collect_config_chain(path: &Path) -> Result<(Value, Vec<PathBuf>), ConfigError> {
    let mut stack = Vec::new();
    let mut visiting = HashSet::new();
    let mut config_chain = Vec::new();
    let value = load_value(path, 0, &mut stack, &mut visiting, &mut config_chain)?;
    Ok((value, config_chain))
}

/// Loads env files from the config chain into the process environment.
///
/// Root values take precedence because `dotenvy::from_path` does not
/// override existing vars. Missing env files are silently skipped.
pub(crate) fn load_env_chain(config_chain: &[PathBuf]) {
    for config_path in config_chain {
        let env_path = config_path.with_extension("env");
        if env_path.exists() {
            let _ = dotenvy::from_path(&env_path);
        }
    }
}
```

Then `load_path` becomes:

```rust
pub(crate) fn load_path(path: &Path) -> Result<Config, ConfigError> {
    let (value, config_chain) = collect_config_chain(path)?;
    load_env_chain(&config_chain);
    Config::from_value(value)
}
```

### 6. Tests

Tests go in `profile/tests.rs` (or `profile.rs` `#[cfg(test)]` - match existing convention). Use `tempfile` for temp directories.

**a) Chain ordering** - `collect_config_chain` returns paths in root-first depth-first order:
- Create `root.toml` including `child.toml`, `child.toml` including `grandchild.toml`
- Assert chain is `[root, child, grandchild]`

**b) Single config, env file present** - env vars are set:
- Create `solo.toml` (valid minimal gateway config) and `solo.env` with `PFG_TEST_SOLO=hello`
- Call `load_path`
- Assert `std::env::var("PFG_TEST_SOLO")` returns `"hello"`
- Clean up with `std::env::remove_var`

**c) Layered precedence** - root env wins over included env:
- Create `root.toml` (includes `parent.toml`), `root.env` with `PFG_TEST_KEY=root_val`, `parent.env` with `PFG_TEST_KEY=parent_val` and `PFG_TEST_EXTRA=extra_val`
- Call `load_path` on root
- Assert `PFG_TEST_KEY` is `"root_val"` (root wins)
- Assert `PFG_TEST_EXTRA` is `"extra_val"` (parent default filled in)
- Clean up

**d) No env file** - load succeeds silently:
- Create `bare.toml` with no matching `bare.env`
- Call `load_path` - no error, no panic

**e) Partial chain** - some configs have env files, some don't:
- Create `root.toml` (includes `mid.toml` includes `leaf.toml`)
- Only `leaf.env` exists with `PFG_TEST_LEAF=leaf_val`
- Assert `PFG_TEST_LEAF` is `"leaf_val"`
- Clean up

**f) Process env takes precedence over all env files:
- Set `PFG_TEST_PROC=process_val` via `std::env::set_var` before calling `load_path`
- `root.env` has `PFG_TEST_PROC=file_val`
- Assert `PFG_TEST_PROC` is still `"process_val"`
- Clean up

### 7. Build, lint, and verify

- `cargo fmt --all --check` - formatting clean
- `cargo clippy -p promptforge-gateway --all-targets -- -D warnings` - no lint warnings
- `cargo build -p promptforge-gateway` - compiles
- `cargo test -p promptforge-gateway` - all tests pass (existing + new)

Per the Rust rulebook: run fmt and clippy before committing. Tests go in `#[cfg(test)] mod tests` in the same file as the code (`profile.rs`), with `use super::*;`. Use `tempfile::TempDir` for filesystem tests (already a dev-dependency).

## What does NOT change

- `config/interpolate.rs` - untouched, still calls `std::env::var()`
- `gateway.toml` - no new syntax, `${VAR}` works exactly as before
- `profile.rs` - include resolution logic untouched, `load_value` just gains a path collector parameter
- Dev runner and MCP server - they talk to the gateway, they don't hold secrets

## Scope note

Only the gateway gets this change. The dev runner needs `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_KEY` (not vendor API keys) - those are low-sensitivity and fine in the shell environment or a future follow-up.