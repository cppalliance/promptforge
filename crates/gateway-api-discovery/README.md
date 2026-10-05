# Gateway discovery

This crate lets a program find and attach to a running gateway instead of starting a second one. The gateway writes a discovery record once it is listening, and readers validate that record, clean up stale ones, settle launch races so only one gateway starts, and wait for it to report healthy. It needs no async runtime, so the gateway and Workshop share one contract.
