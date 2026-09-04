# gateway-config

Typed, validated configuration for the PromptForge gateway. Tooling can read and edit one `gateway.toml` without pulling in the gateway HTTP or runtime stack.

## Version 2 layout

Version 2 is a hard break from profile files and include chains. One file owns global settings, the complete model catalog, and pure-checklist profiles:

```toml
config-version = 2

[server]
bind = "127.0.0.1:8081"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"

[workshop.stt]
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
3. `[workshop]`, `[workshop.stt]`
4. `[local]`
5. `[tools]` and child tables
6. `[[dominion]]`
7. `[[endpoint]]`
8. `[[model]]`
9. `[[local_model]]` and companion tables
10. `[[stt_model]]`
11. `[[profile]]`

`include`, a sibling `profiles/` directory, the top-level `models` allowlist, and `[workshop.voice]` are rejected. Hard-break diagnostics name the file, removed key, source line, and replacement layout.

## Profile selection

The active profile is state, not config. The sibling state file for `gateway.toml` is `gateway.state.toml` and has exactly this shape:

```toml
active_profile = "work"
```

Startup precedence is command-line `--profile`, then `PROMPTFORGE_PROFILE`, then sibling state. The caller supplies the first two values at the config boundary:

```no_run
use gateway_config::{Config, ProfileSelection};
use std::path::Path;

let inputs = ProfileSelection::new(Some("work"), std::env::var("PROMPTFORGE_PROFILE").ok().as_deref());
let config = Config::load(Path::new("gateway.toml"), &inputs)?;
# Ok::<(), gateway_config::ConfigError>(())
```

`Config::from_toml_str` validates an unselected in-memory catalog. `Config::select_profile` derives active remote, local, and STT subsets from that validated catalog without reading disk.

## Validation

Loading validates every profile, not only the active one:

- Profile names are unique and `ProfileName`-legal.
- Every checklist name resolves across `[[model]]`, `[[local_model]]`, and `[[stt_model]]`.
- Every selected local and STT model fits its local dominion VRAM budget.
- Each profile selects at most one interim and one final STT model.
- Interim-only STT is allowed as degraded mode.
- Final-only STT is rejected because streaming requires an interim model.

The built-in `RECOMMENDED_STT_MODELS` pair is `base.en` for interim and `small.en` for final. Both use canonical whisper.cpp URLs and SHA-256 pins from Hugging Face LFS metadata. The ignored live test downloads both artifacts to detect URL or digest drift.

## Pending edits

`save_config_shadow` accepts the pending admin document. It writes global config to `gateway.toml.next` and writes the matching `active_profile` key to `gateway.state.toml.next`. `load_pending_config` reads those shadows with the same selection precedence. No save touches a real file until `promote_shadow` renames the shadow into place, or a caller holding the intended contents commits them with `write_atomic`, the replace-through-rename primitive both shadows and `persist_profile_state` build on.

## License

BSL-1.0. See the repository root for details.
