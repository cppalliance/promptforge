# Introduction

PromptForge turns Markdown files into executable AI prompt pipelines. You write a prompt as a document - YAML frontmatter for metadata, embedded Lua for logic, prose blocks for model instructions - and the system parses, validates, and executes it against any OpenAI-compatible endpoint.

A PromptForge prompt is a single file. Sections run top to bottom. Lua blocks control flow, bind models, declare tools, and write to a virtual filesystem. The model sees the prose. The tool loop dispatches calls and feeds results back. Fanout maps a worker section over a list in parallel. The result is a string.

## Chapters

Each chapter is the user guide for one component of the workspace:

- [Gateway Configuration](gateway-config.md) - configuring the gateway: endpoints, credentials, and model routing.
- [Local Models](local-models.md) - running local GGUF models through the gateway.
- [Speech-to-Text](stt.md) - the `gateway-stt` speech-to-text component.
- [Gateway](gateway.md) - the model backend server: chat completion routing, credentials, and the model catalog.
- [Writing Prompts](prompts.md) - the `promptforge-core` library: prompt file format, Lua scripting, models, tools, fanout, store, and errors.
- [Tool Picker](tool-picker.md) - semantic tool resolution using an embedded embedding model.
- [Web Fetch](webfetch.md) - the built-in web fetch tool: page retrieval, readable-content extraction, SSRF guards.

## How to read this guide

Start with [Writing Prompts](prompts.md) for the prompt file format. If you are deploying a model backend, read [Gateway Configuration](gateway-config.md) and [Gateway](gateway.md). The remaining chapters cover each subsystem in depth. Use the Workshop desktop app to edit and run prompts locally.
