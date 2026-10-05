# Local inference

This crate runs models on the user's own machine for the gateway. It downloads and verifies pinned model files and inference server builds, chooses each model's chat template, and supervises one inference server process per local model. It holds no HTTP routes and makes no profile decisions, because the gateway owns both.
