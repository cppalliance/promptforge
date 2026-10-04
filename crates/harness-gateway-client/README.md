# Harness gateway client

This crate is the standard way a Host connects the Harness to the PromptForge gateway. It supplies the inference broker that runs each model round through the gateway, which can stream a reply to the Host as it forms, and the search provider that runs web searches through the gateway. Its wire code opens no connections and reads no clocks, so another broker can reuse it, and credentials never appear in logs or error text.
