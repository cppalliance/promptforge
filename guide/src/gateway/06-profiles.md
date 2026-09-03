# Profiles and Switching

This chapter teaches you profiles: named checklists that decide which models the gateway serves, and how to switch between them at runtime. Profiles are how one config file serves a work machine, a travel laptop, and a demo box without editing a single model entry.

## Define a profile

A profile is a `[[profile]]` entry that owns only a `name` and a `models` list:

````
[[profile]]
name = "work"
models = ["gpt-5", "qwen3-local", "whisper-base-en", "whisper-small-en"]

[[profile]]
name = "travel"
models = ["gpt-5"]
````

Membership alone decides which models route, spawn, or load. Profiles carry no per-field overrides. A profile selects a subset of the catalog across remote, local, and STT models, and every name it lists must exist exactly once. Duplicate profile names and duplicate members fail validation.

Profile names must be a single safe path component: no surrounding whitespace, not empty, not `.` or `..`, and no path separators. One spelling works in URLs, state files, and labels.

Every profile is validated at load. Names are unique and legal, every listed model exists, and the local and STT subsets are checked against dominion VRAM budgets. A live switch can never land on an invalid profile.

## Where the active profile lives

The active profile lives in a sibling state file, not in the config. A `gateway.toml` maps to a `gateway.state.toml` holding one canonical key:

````
active_profile = "work"
````

The selection survives restarts.

At startup the profile is chosen by precedence: the `--profile` command-line flag, then the `PROMPTFORGE_PROFILE` environment variable, then the sibling state file. With none set, startup refuses and lists the defined profiles. A stale state file naming a deleted profile fails startup with an error naming the stale value and the defined profiles.

## Switch at runtime

Switch the active profile over HTTP:

````
curl -X POST -H "Authorization: Bearer $GATEWAY_KEY" \
  -H "Content-Type: application/json" \
  -d '{"profile": "travel"}' \
  http://127.0.0.1:8081/admin/switch-profile
````

The switch streams its stages as a live SSE event stream: `loading-profile`, `stopping-models`, `starting-models`, and one terminal event. The choice persists to the state file, and the switch runs to completion even if the client disconnects. Switching uses the in-memory catalog; the config file is never re-read from disk.

Activating a profile narrows the served remote, local, and STT catalogs to that profile's member list. Selecting an undefined profile fails with the list of defined profiles.

In-flight inference requests get a bounded drain of up to 30 seconds during a switch. Stragglers are then cancelled, and a caller cancelled this way receives a dedicated error. Switching tears down the old profile's children deterministically, and their VRAM is freed before the replacement profile starts. When a switch starts only some local models, the terminal event names which models loaded and which failed.

