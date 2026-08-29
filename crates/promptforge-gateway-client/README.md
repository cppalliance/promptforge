# promptforge-gateway-client

The PromptForge gateway's model client: an `OpenAI`-compatible chat
completions transport (`GatewayClient`), the wire types it exchanges, the
model catalog (`ModelCatalog`, `ModelDescriptor`, `ModelId`), and the
prompt-local binding vocabulary (`ModelBinding`, `ModelSet`, `ModelView`,
`ModelResolver`) the executor resolves `models.bind` declarations against.

The client holds only the gateway's URL and the shared key; the vendor
credential lives in the gateway, so a caller never sees it. Streaming is not
supported.
