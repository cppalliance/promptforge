# Loopback wall

This crate provides the middleware that keeps the gateway's local-only surfaces local. One check refuses any caller that is not on the same machine, and the other refuses any request that does not name the bound loopback address, which blocks DNS rebinding. The gateway applies both in every build, including builds without the settings page.
