# Individual commits

## 2026-07-28 Create workspace with prompt parser and one-shot executor

## 2026-07-29 Embed sandboxed Lua and run section chunks before model

## 2026-07-29 Walk top-level sections with fall-through in execute::run

## 2026-07-29 core: render MAX_TOOL_ITERATIONS as code in run docs

## 2026-07-31 chore: apply workspace style and lint conformance sweep

## 2026-07-31 chore: gitignore vibe-review and conformance-audit files

## 2026-08-07 core: open the shared library with a plain lua fence

## 2026-08-07 core: run all section VMs through the validating tools path

## 2026-08-07 dev: dump the run store beside the prompt after each run

## 2026-08-08 chore: gitignore prompt store dump directories

## 2026-08-08 dev: clear the store dump directory before each run

## 2026-08-08 docs: add portrait image to the README

## 2026-08-08 docs: add the briefer demo prompt

## 2026-08-08 picker: bind a solo candidate below the strict floor

## 2026-08-08 core: map Lua runtime errors to prompt source lines

## 2026-08-08 Parse positional tool_code args and inject a tool guide

## 2026-08-08 Detect the OpenAI dialect from ChatML tool templates

## 2026-08-09 Add chat_template_file and detect Mistral tool templates

## 2026-08-10 Rebuild promptforge-core around a validated bounded API

## 2026-08-12 Expand README into a full project page with banners

## 2026-08-14 Delete stale docs and fix their references

## 2026-08-15 Move local files under local/ and docs under guide/

## 2026-08-16 Rename gateway and MCP credentials to api_key

## 2026-08-17 Correct promptforge version in doc examples to 1

## 2026-08-18 Fix gateway doctests to set api_key in [server]

## 2026-08-19 Delete the list-h3-with-lua invalid-prompt fixture

## 2026-08-19 Move design documents into one design/ directory

## 2026-08-21 Upgrade h2 to 0.4.18 and indicatif to 0.18

## 2026-08-23 Use as_chunks in the tool-picker build script

pre-existing clippy 1.98 breakage blocked step 1; vibe rule 7 gives old bugs their own commit to keep refactor commits clean. (assistant reasoned, rulebook policy)

## 2026-08-23 Return a ready future from list_tools

surfaced by the full-workspace clippy Verify after per-crate passes; pre-existing on master, committed separately per rule 7. (assistant reasoned)

## 2026-08-23 Normalize CRLF in golden tool-list tests

git checks golden files out as CRLF on Windows while serde_json emits LF; normalize line endings in the comparison since the JSON payload is identical either way. (assistant reasoned)

## 2026-08-24 Remove an unused glob import from debug tests

the conflict resolution deleted a dialect test, orphaning a glob import - a defect in neither parent, purely a merge artifact; committed on top rather than amended because the rebase was done. (user said "fix the rebase", assistant reasoned placement)

## 2026-08-24 Demote private doc links in execute module docs

post-rebase cargo doc failed on private intra-doc links introduced by upstream's refactor; demote to plain text as a rule-7 fix. (assistant reasoned)

## 2026-08-24 Add workbench design doc, archive prior designs

## 2026-08-24 Show interim voice text in textarea; sustain thinking LED

one bundled commit of dictation-UX direction given verbatim by the user: interim text lives in the text box and grows it, the status bar is full-width and top-level, REC is a maroon badge with priority over the LED, and the LED means amber-while-thinking, green-on-output-spurt. (user said)

## 2026-08-24 Suppress the LIBCMT defaultlib warning on MSVC

whisper-rs-sys builds whisper.cpp with the static CRT while Rust links the dynamic CRT; the linker resolves it but warns, so /NODEFAULTLIB:LIBCMT is scoped to the MSVC target. (assistant reasoned)

