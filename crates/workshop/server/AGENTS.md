# workshop-server

- Do not install global tracing state or retain process-global initialization that ignores the embedding binary's arguments.
- Do not pass one subsystem's handles into another subsystem's constructor, and do not reintroduce per-request panics on missing registrations.
