# Introduction

PromptForge starts from one premise: human intent is the source code, and everything downstream of it - plans, prompts, reports - is a build artifact. Source is versioned; artifacts are regenerated. Regeneration costs a run, not a reconstruction.

This ordering follows from scarcity. Human judgment is the scarce resource; model output is abundant. So models advise and compare, and humans decide. Every design decision in the product traces back to that ordering.

The system protects the judgment you invest in two ways. It compiles the structural rules of your methodology into the runtime, so a rule cannot be forgotten under context pressure. And it records every run, edit, decision, and mistake in an append-only event store, so the judgment that built a pipeline is never lost.

## The moving parts

The system has four parts, and the engine is the center.

The engine is a Rust library. It parses a Markdown prompt file and executes it as a program against any OpenAI-compatible endpoint. It gives you deterministic control flow, isolated sections, and engine-controlled fan-out.

The gateway is the one process that talks to model backends. It holds every credential, routes chat completions by capability name, manages the model catalog, and runs local models on your own hardware.

The Workshop is a standalone local desktop application. It wraps the engine in an environment where every run, edit, decision, and mistake is recorded in an append-only, hash-chained event store.

The library is the engine packaged as a dependency. An integrator embeds prompt execution in their own program; the Workshop itself is built on the library.

The parts connect in one direction. The Workshop and the library sit on the engine. The engine talks only to the gateway. The gateway fronts every model backend, local or frontier.

## Which set is yours

Each audience has one documentation set.

If you use the Workshop desktop application, read [the Workshop set](workshop/index.md). It teaches the workbench, the chat surface, the editor, voice input, models and profiles, and updates.

If you operate the gateway, read [the Gateway set](gateway/index.md). It teaches installation, the configuration file, remote and local models, speech-to-text, profiles, and the operational surface.

If you write prompts, read [the Prompt Language set](language/index.md). It teaches the .md prompt syntax: frontmatter, sections and blocks, Lua globals, prose substitution, models, tools, control flow, and fanout.

If you write agent programs, read [the Agent Programs set](agent/index.md). It teaches the .lua host surface: the agent loop, chat rounds, tool calls, the event log, host state, the sandbox, and the full loop.

