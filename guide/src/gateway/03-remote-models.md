# Remote Models and Endpoints

This chapter teaches you to declare remote backends and the models they serve. You will learn endpoint entries, model entries, and the catalog your callers see. Remote models are the simplest way to get the gateway serving, so they come first.

## Declare an endpoint

A remote backend is a `[[endpoint]]` entry. Start with one:

````
[[endpoint]]
id = "openai"
protocol = "openai"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"
````

Each entry has an `id`, a `protocol` of `openai`, a `base_url`, an `api_key`, and an optional `dominion` binding. A trailing slash on the base URL is trimmed. Endpoint ids must be non-empty and unique. Each `base_url` must be an absolute http or https URL with a host; values like `not-a-url` or `ftp://example.com` fail validation.

## Declare a model

A remote model is a `[[model]]` entry that maps a public name to the alias the backend knows:

````
[[model]]
name = "gpt-5"
kind = "chat"
description = "GPT-5 via OpenAI"
context = 272000
thinking = "switchable"
upstream = "gpt-5"
endpoints = ["openai"]
````

Each entry has a `name`, a `kind`, a `description`, a `context` size, a `thinking` mode, an `upstream` alias, a list of `endpoints`, an optional `default_max_tokens`, and an optional `tool_dialect`. The upstream alias is the string the backend knows the model by.

Every remote model must list at least one endpoint, and every endpoint it names must be defined. Model names must be unique across remote and local models, so one name always refers to one model.

## Kinds and thinking modes

Every model carries a `kind`: `chat`, `embedding`, or `classifier`. The kind scopes which fields are meaningful. Chat-only fields such as `thinking` and `default_max_tokens` are rejected for non-chat kinds at load time.

Record each chat model's thinking behavior as `never`, `always`, or `switchable`. Switchable means the client may toggle thinking per request.

## Tool dialects

The default `openai` tool dialect forwards tool definitions to the backend verbatim. For a backend without a native tool array, set the emulating dialect on a chat model:

````
tool_dialect = "gemma3_tool_code"
````

With this dialect the gateway injects a tool guide into the system prompt, strips the tool fields from the outgoing request, and parses tool fences from the reply.

## Advertise capabilities

You can advertise per-model capabilities that surface verbatim on GET /v1/models, so clients can shape requests before sending them:

````
[model.capabilities]
max_output = 16384
default_temperature = 1.0
images = true
parallel_tool_calls = true
effort_levels = ["low", "medium", "high"]
default_effort = "medium"
adaptive_thinking = true
````

The capability fields are `max_output`, `default_temperature`, `images`, `parallel_tool_calls`, `effort_levels`, `default_effort`, and `adaptive_thinking`. They obey cross-field rules at load time. A `default_effort` without `effort_levels` fails. A `default_effort` not listed in `effort_levels` fails. Effort fields fail when thinking is `never`. A `max_output` larger than `context` fails; an exact fit passes.

Enumerated fields accept a fixed spelling vocabulary. Use the spellings verbatim: protocol `openai`; thinking `never`, `always`, or `switchable`; tool_dialect `openai` or `gemma3_tool_code`; model kind `chat`, `embedding`, or `classifier`.

## What the caller sees

Callers observe the catalog at GET /v1/models:

````
curl -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/v1/models
````

Each configured model carries its caller-facing id, its workload kind, its description, its context window size, its thinking mode, and its capability metadata.

When a caller sends a chat, embedding, or rerank request, the gateway forwards it to the backend paths `chat/completions`, `embeddings`, or `rerank` relative to the configured base URL. The public model name is rewritten to the upstream alias. The caller's bearer token is never sent upstream.

