# Harness capabilities

This crate is the Harness's capability layer: the registry a Host fills with capabilities, the per-run activation that checks for conflicts and assembles the tool catalog, and the traits every capability and tool implements. It also defines how a capability asks a person a question through the Host, and it provides the built-in user input capability. It is private to the Harness family, and clients reach it through the Harness's public API.
