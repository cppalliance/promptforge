# Gateway configuration

This crate reads, validates, and writes the gateway's configuration and its active profile selection, so tools can inspect or edit a configuration without the gateway's server code. Loading validates every profile, not only the active one, and pending edits stay in a shadow copy until they are applied.
