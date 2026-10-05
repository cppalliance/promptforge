# Harness Plugins

This crate is the Harness's Plugin layer: the registry a Host fills with Plugins, the per-run activation that checks for conflicts and assembles the tool catalog, and the traits every Plugin and tool implements. It also defines how a Plugin asks a person a question through the Host, and it provides the built-in user input Plugin. It is private to the Harness family, and clients reach it through the Harness's public API.
