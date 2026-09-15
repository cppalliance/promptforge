# gateway-config

Typed, validated configuration for the PromptForge gateway. Tooling can read and edit one `gateway.toml` without pulling in the gateway HTTP or runtime stack.

## Version 2 layout

Version 2 is a hard break from profile files and include chains. One file owns global settings, the complete model catalog, and pure-checklist profiles:

```toml
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"

[stt]
window_seconds = 15
interval_ms = 500

[[stt_model]]
name = "whisper-base-en"
role = "interim"
source = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
sha256 = "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002"
vram_gb = 1.0

[[profile]]
name = "work"
models = ["whisper-base-en"]
```

Use this canonical section order to minimize merge noise:

1. `config-version`
2. `[server]`
3. `[stt]`
4. `[workshop]`
5. `[local]`
6. `[tools]` and child tables
7. `[[dominion]]`
8. `[[endpoint]]`
9. `[[model]]`
10. `[[local_model]]` and companion tables
11. `[[stt_model]]`
12. `[[profile]]`

Version 2 accepts only the canonical top-level `[stt]` section. Legacy `[workshop.stt]` input, including documents that also define `[stt]`, is rejected as an unknown workshop field.

`include`, a sibling `profiles/` directory, the top-level `models` allowlist, and `[workshop.voice]` are rejected. Hard-break diagnostics name the file, removed key, source line, and replacement layout.

## Profile selection

The active profile is state, not config. The sibling state file for `gateway.toml` is `gateway.state.toml` and has exactly this shape:

```toml
active_profile = "work"
```

An absent state file selects no profile, which is a legal load: `Config::load` returns `Ok` with `active_profile() == None` and empty local and STT selections, and every `[[model]]` still routes. Startup precedence is command-line `--profile`, then `PROMPTFORGE_PROFILE`, then sibling state. The first two are ephemeral and never write the state file. When the state file wins and names a profile the document does not define, the load degrades to no profile and records the stale name behind `Config::stale_state_selection()`; when the command line or environment names an undefined profile, the load fails. A malformed name from any source fails. `persist_profile_state` writes the state file and `clear_profile_state` deletes it (an absent file is success). The caller supplies the ephemeral values at the config boundary:

```no_run
use gateway_config::{Config, ProfileSelection};
use std::path::Path;

let inputs = ProfileSelection::new(Some("work"), std::env::var("PROMPTFORGE_PROFILE").ok().as_deref());
let config = Config::load(Path::new("gateway.toml"), &inputs)?;
# Ok::<(), gateway_config::ConfigError>(())
```

`Config::from_toml_str` validates an unselected in-memory catalog. `Config::select_profile` derives the active local and STT subsets from that validated catalog without reading disk; it accepts `None` for no profile and never narrows `models()`, which is the full remote catalog under every selection.

## Validation

Loading validates every profile, not only the active one:

- Profile names are unique and `ProfileName`-legal.
- Every checklist name resolves to a `[[local_model]]` or `[[stt_model]]` entry. A name that resolves to a `[[model]]` is rejected as a remote model, because profiles never gate remote routing; an unknown name is rejected as undefined.
- Every selected local and STT model fits its local dominion VRAM budget.
- Each profile selects at most one interim and one final STT model.
- Interim-only STT is allowed as degraded mode.
- Final-only STT is rejected because streaming requires an interim model.

The built-in `RECOMMENDED_STT_MODELS` pair is `base.en` for interim and `small.en` for final. Both use canonical whisper.cpp URLs and SHA-256 pins from Hugging Face LFS metadata. The ignored live test downloads both artifacts to detect URL or digest drift.

`realtime-transcribe` is reserved for the Gateway's logical Realtime model and cannot be used as a physical `[[stt_model]]` name. The Gateway advertises that logical name only while an interim and final pair is active; physical names remain the batch transcription selectors.

## Pending edits

`save_config_shadow` accepts the pending admin document and writes it to `gateway.toml.next`, the only shadow. A document carrying `active_profile` fails validation with a message pointing at `POST /admin/switch-profile`: the selection is state, never staged. `load_pending_config(config_path, selection)` reads the config shadow when present and resolves the profile exactly as `Config::load` does, from the ephemeral selection and the real `gateway.state.toml`. `pending_report` lists only the config shadow. No save touches a real file until `promote_shadow` renames the shadow into place, or a caller holding the intended contents commits them with `write_atomic`, the replace-through-rename primitive the shadow and `persist_profile_state` build on.

## License

BSL-1.0. See the repository root for details.
