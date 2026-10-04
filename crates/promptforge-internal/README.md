# Engine internals

This directory holds the Engine's private crates, which together parse a prompt program and step its run. Code outside the Engine reaches them only through the Engine's public API crate, the one crate allowed to depend on this directory.
