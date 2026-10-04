# Gateway

The gateway is the server process that holds every model credential. It serves an OpenAI-compatible API, routes each request to a cloud provider or a local model, manages the model catalog and local model downloads, and, when built with them, serves web search, speech, and a browser settings page. It starts serving before any slow model work begins, and on desktop machines it runs from the system tray. The [gateway guide](https://cppalliance.org/promptforge/gateway/) documents its configuration, profiles, and endpoints.
