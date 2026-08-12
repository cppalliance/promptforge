# Introduction

PromptForge turns Markdown files into executable AI prompt pipelines. You write a prompt as a document - YAML frontmatter for metadata, embedded Lua for logic, prose blocks for model instructions - and the system parses, validates, and executes it against any OpenAI-compatible endpoint.

A PromptForge prompt is a single file. Sections run top to bottom. Lua blocks control flow, bind models, declare tools, and write to a virtual filesystem. The model sees the prose. The tool loop dispatches calls and feeds results back. Fanout maps a worker section over a list in parallel. The result is a string.

## Components

PromptForge is a workspace of cooperating crates:

| Crate | What it does |
|-------|-------------|
| **promptforge-cli** | Command-line tool. Point it at a prompt file and run it. |
| **promptforge-gateway** | Model backend server. Routes chat completions to configured LLM endpoints, manages credentials, serves a model catalog, and optionally runs local GGUF models. |
| **promptforge-core** | The library. Parser, execution engine, Lua sandbox, model resolution, tool dispatch, fanout, virtual store. Everything above depends on this. |
| **promptforge-mcp-server** | Serves prompts as MCP tools for agentic harnesses like Cursor and Claude Code. |
| **promptforge-tool-picker** | Semantic tool resolution. Matches capability descriptions to concrete tools using an embedded embedding model. |
| **promptforge-webfetch** | Built-in web fetch tool. Retrieves pages, extracts readable content, guards against SSRF. |
| **promptforge-dev** | Interactive development runner. Watch mode, store dump inspection, raw trace capture. |

## How to read this guide

This guide follows the user journey. Start with [Getting Started](getting-started.md) to run your first prompt. Then read [Prompt Files](prompt-files.md) and [Lua Scripting](lua.md) to understand the format. The remaining chapters cover each subsystem in depth.

If you are integrating PromptForge as a library, the [Execution](execution.md) chapter explains the programmatic API. If you are deploying a model backend, start with [Gateway](gateway.md).
