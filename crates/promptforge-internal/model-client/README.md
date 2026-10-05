# Engine model vocabulary

This crate defines the messages, tool schemas, completions, and failures a model round exchanges, along with the model catalog and the prompt-local model bindings the executor resolves. Its constructors check every reply the same way, whoever built it. It holds no transport and no wire parsing, which live in the Harness's gateway client.
