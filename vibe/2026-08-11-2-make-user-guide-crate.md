---
name: make-user-guide crate
overview: Create a new `make-user-guide` binary crate that reads the per-crate user guides and assembles them into a single `promptforge-user-guide.md` at the repo root.
todos:
  - id: create-crate
    content: Create crates/make-user-guide/ with Cargo.toml and src/main.rs
    status: completed
  - id: verify-build
    content: Build and run the program, verify output
    status: completed
isProject: false
---

# make-user-guide Crate

Create `crates/make-user-guide/` - a private, zero-dependency binary crate that reads the 7 per-crate user guides and assembles `promptforge-user-guide.md` at the workspace root.

## Crate structure

```
crates/make-user-guide/
  Cargo.toml
  src/main.rs
  user-guide-1.md    # top: title, intro (assembled first)
  user-guide-2.md    # bottom: closing matter (assembled last)
```

## Cargo.toml

```toml
[package]
name = "make-user-guide"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]

[lints]
workspace = true
```

No dependencies. Pure `std`. `publish = false` keeps it out of crates.io. Auto-discovered by the workspace's `members = ["crates/*"]`.

## Assembly logic (`src/main.rs`)

The program:

1. Locates the workspace root by walking up from `env!("CARGO_MANIFEST_DIR")` to find the directory containing `Cargo.toml` with `[workspace]`
2. Defines an ordered list of `(crate_name, guide_filename)` pairs, in user-journey order (what you touch first, then deeper):
   - `promptforge-cli` / `user-guide-promptforge-cli.md`
   - `promptforge-gateway` / `user-guide-promptforge-gateway.md`
   - `promptforge-core` / `user-guide-promptforge-core.md`
   - `promptforge-mcp-server` / `user-guide-promptforge-mcp-server.md`
   - `promptforge-tool-picker` / `user-guide-promptforge-tool-picker.md`
   - `promptforge-webfetch` / `user-guide-promptforge-webfetch.md`
   - `promptforge-dev` / `user-guide-promptforge-dev.md`
3. For each guide file, reads the content and **demotes all markdown headings by one level** (H1 becomes H2, H2 becomes H3, etc.) so the combined document has one top-level H1
4. Assembles the output in this order:
   - `crates/make-user-guide/user-guide-1.md` (verbatim, no heading demotion)
   - Each crate's guide (heading-demoted), separated by `---`
   - `crates/make-user-guide/user-guide-2.md` (verbatim, no heading demotion)
5. Writes the result to `{workspace_root}/promptforge-user-guide.md`
6. Prints the output path and exits 0. On any missing file, prints the path and exits 1.

Both `user-guide-1.md` and `user-guide-2.md` ship with small placeholders. The human edits them later.

## Heading demotion

Simple line-by-line processing: any line starting with `#` gets one `#` prepended. This handles H1-H6 correctly and doesn't touch `#` inside code fences. The program tracks fence state (inside/outside a code block) to avoid modifying heading-like lines inside fenced blocks.

## Workspace integration

The workspace [Cargo.toml](promptforge/Cargo.toml) already uses `members = ["crates/*"]`, so the new crate is auto-discovered. No edit to the workspace file needed.
