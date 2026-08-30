# promptforge-gateway-config

Configuration loading for the PromptForge inference gateway, factored into a
standalone crate so tooling (config editors, IDE integrations, CLIs) can read
and validate a `gateway.toml` without pulling in the gateway's HTTP stack.

What it provides:

- [`Config`](src/config.rs): the validated, whole-gateway configuration.
  Construction is only possible through `Config::load`, `Config::load_profile`,
  or `Config::from_toml_str`, each of which runs `${VAR}` interpolation and
  full semantic validation, so a `Config` value cannot hold an invalid state.
- Named profiles: recursive `include` resolution (depth-first, relative to the
  including file, with cycle detection), keyed-array merging by `id`/`name`,
  and the [`ProfileName`](src/profile/name.rs) confinement type that keeps a
  profile selection inside the profiles directory.
- The boot-only `[workshop]` section ([`WorkshopConfig`](src/config/workshop.rs)):
  the hosted workshop UI's bind and optional voice/tape sub-tables, with
  tape-path anchoring against the boot-config directory, a loopback-adjusted
  client URL derived from `[server]` (`ServerConfig::client_url`), and
  `load_workshop` for reading the section without full validation.
  `load_boot_sections` reads `[server]` and `[workshop]` together in a
  single include-resolution pass.
- [`Secret`](src/config.rs): a credential wrapper that redacts in `Debug` and
  `Display` and serializes only as the `"***"` marker.
- `Config::to_json`: the resolved configuration rendered as a JSON object in
  the TOML shape, with each keyed-array entry (`[[model]]`, `[[local_model]]`,
  `[[endpoint]]`, `[[dominion]]`) annotated with the `source_file` its winning
  definition came from and a top-level `source_files` map of every written
  dotted TOML path - the merge pass records provenance without changing merge
  semantics.
- Local-model companion types ([`SpeculativeConfig`,
  `MultimodalProjectorConfig`, `SpeculationType`, and
  `DraftTokenMax`](src/config/companion.rs)): declarative speculative-decoding
  drafters (`draft-mtp` only, with a bounded `draft_max`) and multimodal
  projectors for chat `[[local_model]]` entries. Companion sources follow the
  artifact-source rules - `https` with a mandatory `sha256` pin, or an
  operator-controlled local path - and are validated before values leave the
  crate.
- Shadow files ([`shadow.rs`](src/profile/shadow.rs)): pending edits stage
  beside the real files (`default.toml` gains `default.toml.next`, never
  `.next.toml`, so the profile listing cannot see a phantom profile).
  `save_profile_shadow`, `save_include_shadow`, and `save_boot_shadow`
  restore `"***"`-redacted secrets from the current state, resolve the
  include chain preferring existing shadows, validate the merged result like
  a real load, and only then write atomically (temp + rename);
  `save_include_shadow` refuses a target the pending chain never reaches
  (it could not be validated); `write_shadow`
  stages arbitrary sibling content such as `.env` files, and `shadow_path`
  names the shadow for any managed file. No save ever touches a real file.
  Reading the pending state back, `load_pending_profile` loads the chain
  with shadows preferred (provenance names the `.next` files), and
  `pending_report` lists which real files carry shadows and which
  top-level sections differ between the real and pending views (an absent
  section equals a vacant one, so serialized empty defaults report no
  phantom change). `promote_shadow` is the explicit apply step: one atomic
  rename makes the shadow the real file, so the shadow disappears and a
  reader observes either the old or the new content, never a mix.
- [`ConfigError`](src/api_error.rs): an opaque, source-preserving error type;
  classify failures with `ConfigError::kind` and `ConfigErrorKind`.

```no_run
use promptforge_gateway_config::{Config, ProfileName};
use std::path::Path;

# fn demo() -> Result<(), promptforge_gateway_config::ConfigError> {
let name = ProfileName::parse("dev").expect("valid profile name");
let config = Config::load_profile(Path::new("profiles"), &name)?;
let _ = config;
# Ok(())
# }
```

## License

BSL-1.0. See the repository root for details.
