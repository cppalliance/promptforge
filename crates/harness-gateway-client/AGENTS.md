# harness-gateway-client

- One request-body builder. `build_request_body` is the only place the chat-completions body is shaped, so every transport sends the same JSON for one `Chat` effect. Never build or patch a body elsewhere.
