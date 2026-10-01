# Introduction

PromptForge starts from one premise: human intent is the source code, and everything downstream of it - plans, prompts, reports - is a build artifact. Source is versioned; artifacts are regenerated. Regeneration costs a run, not a reconstruction.

This ordering follows from scarcity. Human judgment is the scarce resource; model output is abundant. So models advise and compare, and humans decide. Every design decision in the product traces back to that ordering.

The system protects the judgment you invest in two ways. It compiles the structural rules of your methodology into the runtime, so a rule cannot be forgotten under context pressure. And it records every run, edit, decision, and mistake in an append-only event store, so the judgment that built a pipeline is never lost.

## The moving parts

The system has five parts, and the Engine is the center.

The Engine is a Rust library. It parses a Markdown prompt file and executes it as a program, and it emits an effect whenever the program needs a model reply, a tool result, or a file. It gives you deterministic control flow, isolated sections, and Engine-controlled fan-out.

The Harness is the Rust library that runs the Engine. It performs every effect the Engine emits, sends each model request to the gateway, and keeps the run log.

The gateway is the one process that talks to model backends. It holds every credential, routes chat completions by capability name, manages the model catalog, and runs local models on your own hardware.

The Workshop is a standalone local desktop application and a Host, an application that runs prompts through the Harness. It wraps the Harness in an environment where every run, edit, decision, and mistake is recorded in an append-only, hash-chained event store.

The library is the Engine and the Harness packaged as dependencies. An integrator embeds prompt execution in their own program, which makes that program a Host; the Workshop itself is built on the library.

The parts connect in one direction. The Workshop and every other Host sit on the Harness, and the Harness sits on the Engine. The Harness talks to the gateway for every model reply. The gateway fronts every model backend, local or frontier.

## Which set is yours

Each audience has one documentation set.

If you operate the gateway, read [the Gateway set](gateway/index.md). It teaches installation, the configuration file, remote and local models, speech-to-text, profiles, and the operational surface.

If you write prompts, read [the Prompt Language set](language/index.md). It teaches the .md prompt language in full: file structure, blocks and prose, how a prompt runs, the Lua environment, arguments and substitution, jump and call, the store, models and conversations, tools and web access, fanout, tasks and task events, and limits and errors, with a quick reference at the end.

