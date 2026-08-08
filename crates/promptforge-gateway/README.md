# `promptforge-gateway`

Inference gateway binary: holds backend credentials, routes OpenAI-shaped chat completions, and advertises a bearer-authed model catalog.

## What it serves

- `POST /v1/chat/completions` - OpenAI-shaped passthrough; `temperature`, `max_tokens`, and `chat_template_kwargs` ride through the request body's catch-all
- `GET /v1/models` - catalog built from every `[[model]]` in `gateway.toml` (`id`, `description`, `context`, `thinking`)
- `POST /v1/tools/web_search` - optional Brave-backed search when `[tools.web_search]` is configured
- `GET /health` - unauthenticated liveness

Hosts fetch `GET /v1/models` to build the `ModelCatalog` that H1 `models.need` binds against. Chat remains passthrough; catalog metadata does not rewrite request bodies.

## Configuration

See the repository-root `gateway.toml` and the root README "Gateway configuration" section for the full `[[endpoint]]` / `[[model]]` shape, including required `description` and `context`, and `thinking` of `never`, `always`, or `switchable`.

```text
cargo run -p promptforge-gateway -- serve gateway.toml
```

`design-gateway.md` is the design document for this crate.
