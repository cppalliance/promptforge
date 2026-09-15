# Profiles and Selection

This chapter teaches you profiles: named checklists that decide which local models the gateway loads, how the selection is stored, and how you change it. Profiles are how one config file serves a work machine, a travel laptop, and a demo box without editing a single model entry.

## Define a profile

A profile is a `[[profile]]` entry that owns only a `name` and a `models` list:

````
[[profile]]
name = "work"
models = ["qwen3-local", "whisper-base-en", "whisper-small-en"]

[[profile]]
name = "travel"
models = ["qwen3-local"]
````

A profile is a checklist of local and speech-to-text models. Membership alone decides which local models spawn and which speech models load; profiles carry no per-field overrides. Every name a profile lists must be a `[[local_model]]` or `[[stt_model]]` entry, and each must exist exactly once. Naming a remote `[[model]]` in a profile fails validation with an error saying the model is remote: remote models are never gated by a profile, because every `[[model]]` in the catalog routes all the time. Duplicate profile names and duplicate members also fail validation.

Profile names must be a single safe path component: no surrounding whitespace, not empty, not `.` or `..`, and no path separators. One spelling works in URLs, state files, and labels.

Every profile is validated at load. Names are unique and legal, every listed model exists, each profile selects at most one interim and one final speech model, and the local and speech subsets are checked against dominion VRAM budgets. The gateway never boots into an invalid profile.

## Where the selection lives

The selected profile lives in a sibling state file, not in the config. A `gateway.toml` maps to a `gateway.state.toml` holding one canonical key:

````
active_profile = "work"
````

The selection survives restarts. An absent state file is the persisted form of "no profile": the gateway boots, serves every remote model, and loads no local or speech models. "No profile" is a selectable state, not an error.

At startup the profile is chosen by precedence: the `--profile` command-line flag, then the `PROMPTFORGE_PROFILE` environment variable, then the sibling state file. The flag and the variable are ephemeral; they never write the state file. With none set, the gateway boots with no profile.

A state file naming a profile the config no longer defines does not stop the boot. The gateway logs a warning naming the stale value and the defined profiles, then boots with no profile. The stale name stays in the state file until you select something else, and the configuration UI shows it as a stale selection. A `--profile` flag or `PROMPTFORGE_PROFILE` value naming an undefined profile is still a startup error, because an operator typed it for this run.

## The local model set is fixed at boot

The gateway loads its local models once, at boot, from the profile it started with. After the listener is bound, one boot command downloads the profile's local model artifacts, spawns the `llama-server` children, publishes each into the routing table as it becomes ready, and performs the process's one speech engine load. While a local model is still downloading or spawning, a request for it gets `503` with code `model_loading` and `Retry-After: 5`, `GET /admin/status` lists it under `loading_models`, and `GET /v1/models` lists only routable models. When only some local models start, the boot reports which loaded and which failed, and the ones that loaded keep serving.

Nothing after boot changes the set of local models. There is no live switch, no drain of in-flight requests, and no stop-and-spawn of children while the gateway serves. Remote models are the exception: every `[[model]]` routes from boot, and an applied edit to the remote catalog reloads routing live, as the next chapter explains.

## Select a profile

Select a profile over HTTP:

````
curl -X POST -H "Authorization: Bearer $GATEWAY_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "travel"}' \
  http://127.0.0.1:8081/admin/switch-profile
````

The request body carries `name`: a profile name, or `null` to select no profile. The gateway checks a named profile against the loaded catalog and refuses an undefined name with the list of defined profiles. It then writes the state file (or deletes it for `null`) and answers plain JSON:

````
{"profile": "travel", "restart_required": true}
````

`restart_required` is true when the selection differs from the profile the process is running. The selection persists at once; the running gateway keeps serving its boot profile until it restarts. Selecting the profile that is already running answers `restart_required: false` and changes nothing. Selection uses the in-memory catalog; the config file is never re-read from disk.

Restart the gateway to load the selection. A gateway the Workshop supervises is restarted by the Workshop when you pick a profile from its Model menu; a gateway you run yourself restarts by hand, and the configuration UI shows a banner reading "Restart the gateway to apply these changes." until the new process comes up.

The selection is not part of the config edit surface. `PUT /admin/config` refuses a document carrying `active_profile`, and `GET /admin/config-dirty` never reports it. `GET /admin/config-pending` reports the persisted selection under `profile.active_profile`, read from the real state file, so a client can show a selection that differs from the running profile or names a profile the config no longer defines.
