# `promptforge-gateway`

Inference gateway binary: holds backend credentials, routes OpenAI-shaped chat completions, and advertises a bearer-authed model catalog.

## What it serves

- `POST /v1/chat/completions` - OpenAI-shaped passthrough; `temperature`, `max_tokens`, and `chat_template_kwargs` ride through the request body's catch-all
- `GET /v1/models` - catalog built from every `[[model]]` / `[[local_model]]` in the active profile
- `POST /v1/tools/web_search` - optional Brave-backed search when `[tools.web_search]` is configured
- `GET /admin/profiles`, `GET /admin/status`, `POST /admin/switch-profile` - named profiles (same bearer as `/v1`)
- `GET /health` - unauthenticated liveness

Hosts fetch `GET /v1/models` to build the `ModelCatalog` that H1 `models.need` binds against. Chat remains passthrough; catalog metadata does not rewrite request bodies.

## Configuration

See the repository-root `gateway.toml`, example profiles under `profiles/`, and `design-gateway.md` for `[[endpoint]]` / `[[model]]` / `[[local_model]]` / `include` / devices.

```text
cargo run -p promptforge-gateway -- serve gateway.toml
cargo run -p promptforge-gateway -- serve --profiles-dir ./profiles --profile analytical
```

`design-gateway.md` is the design document for this crate.
