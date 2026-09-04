# The Configuration File

This chapter teaches you the shape of the one file that configures the whole gateway. You will learn the version key, the server section, how to keep secrets out of the file, when a same-machine caller needs no key, and the loopback wall that guards the admin surface. Every other chapter adds sections to this file, so a solid mental model here pays off everywhere.

## One file, one version

You configure the gateway in a single version-2 `gateway.toml` file. The file owns the global settings, the complete model catalog, and the profiles. The file must declare its version on the first line:

````
config-version = 2
````

Any other version fails to load. There is no silent upgrade path.

## A minimal configuration

A minimal configuration has one `[server]` section, one or more `[[endpoint]]` backends, and one or more `[[model]]` entries that map public names to upstream aliases:

````
config-version = 2

[server]
bind = "127.0.0.1:8081"
api_key = "${GATEWAY_KEY}"

[[endpoint]]
id = "openai"
protocol = "openai"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"

[[model]]
name = "gpt-5"
kind = "chat"
description = "GPT-5 via OpenAI"
context = 272000
thinking = "switchable"
upstream = "gpt-5"
endpoints = ["openai"]
````

The `[server]` section sets the socket address and the shared bearer key. Every request from another machine must present the key, and a key that is presented is always checked. The key must not be empty. A third field, `trust_loopback`, controls whether callers on the gateway's own machine may skip the key. It defaults to true, which on a shared machine also admits every other OS account there; set `trust_loopback = false` to require the key from everyone. The rule is covered in full below.

The model catalog lives in the same file. Remote models are `[[model]]` entries. Local models are `[[local_model]]` entries. Speech models are `[[stt_model]]` entries. Later chapters cover each kind.

Keep the sections in the canonical order: `config-version`, `[server]`, `[workshop]`, `[local]`, `[tools]`, `[[dominion]]`, `[[endpoint]]`, `[[model]]`, `[[local_model]]`, `[[stt_model]]`, `[[profile]]`. The order minimizes merge noise when two people edit the file.

## Keep secrets out of the file

Reference environment variables in string values with `${VAR}` syntax:

````
api_key = "${OPENAI_API_KEY}"
````

A literal dollar sign is written `$$`. Interpolation runs only on string values, after the TOML is parsed, so a variable reference inside a comment or a key is never expanded. An unclosed `${...}` fails the load. A reference to an unset variable fails the load with a distinct error that names the variable.

At startup the gateway loads the config-sibling `.env` file into the process environment before it reads the config. Variables already set in the environment win. A missing variable surfaces later as the unresolved `${VAR}` error.

Secrets never serialize. When the gateway renders the configuration, every secret field shows `***` instead of credential material. You can view the running configuration rendered as JSON in TOML shape, and you can list which config fields reference each `${VAR}` variable; the values are never exposed.

## Validation never lets a bad file load

A configuration never loads without passing validation. Unknown keys in any section are rejected, never ignored. Removed layout features, such as include chains or a sibling profiles directory, fail with hard-break diagnostics that name the file, the removed key, the source line, and the replacement layout. Removed legacy keys such as `[queue]` or an endpoint's `concurrency` fail at parse time. An old config cannot silently load.

You can classify a load failure into stable kinds: unreadable file, invalid TOML, malformed interpolation, unset environment variable, failed semantic check, removed layout feature, or shadow write failure.

Two field rules are worth memorizing early. A `sha256` pin must be exactly 64 hexadecimal characters; uppercase and surrounding whitespace are accepted and normalized to lowercase. And a `[[model]]` entry without a `description` or a `context` is rejected at load.

## Loopback trust

By default a caller on the gateway's own machine needs no key. With `trust_loopback = true` (the default, and what the first-run config writes), a request from a loopback peer that presents no credential at all is admitted on every route, the admin surface included. That is what lets `curl http://127.0.0.1:8081/v1/models`, the SDK with only `PROMPTFORGE_GATEWAY_URL` set, and the config UI on its own origin work without a key.

The trust is narrow on purpose. It applies only when the request carries no `Authorization` header: a presented-but-wrong bearer is still rejected with 401, even from loopback, so a stale key is always detected. And it applies only when the request's fetch metadata allows ambient access: no `Sec-Fetch-Site` header (curl, the SDK, any non-browser client) or a value of `same-origin` or `none` (the config UI, a typed URL). A page on another origin sends `cross-site`, and browsers never let a page strip that header, so a web page cannot ride your loopback peer into the admin surface. A request with no peer address fails closed and needs the key.

The cost is the shared-machine case. On a machine with more than one OS account, any other account can use your gateway, including reading upstream API keys from the admin config surface. If that describes your machine, set `trust_loopback = false` to require the bearer key from every caller, or bind the gateway off loopback:

````
[server]
bind = "127.0.0.1:8081"
api_key = "${GATEWAY_KEY}"
trust_loopback = false
````

`[server]` is process-owned, so a change to `trust_loopback` takes effect on the next restart.

## The loopback wall

The admin config endpoints sit behind a loopback wall in every build. A non-loopback peer gets 403 before bearer auth even runs. The wall covers config read and write, the env file, pending state, apply and revert, orphans, system metrics, model info, chat templates, the Hugging Face proxy, profile create and delete, and reveal. The wall fails closed: a request with no peer address is refused. Loopback trust adds a rule to authentication; it removes no wall.

## Derived addresses

An unspecified bind IP such as 0.0.0.0 or :: becomes the matching loopback address in derived client URLs. Same-host consumers, including a hosted workshop, always get a dialable URL.

