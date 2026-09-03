# Install and Run

This chapter teaches you to get the gateway running on your machine: how to install it, how to start it with a config file and a profile, and how to confirm it is healthy. You do these things every time you bring the gateway up, so they are worth learning well.

## Install the binary

The gateway is a single binary named `gateway`. It serves an OpenAI-shaped inference API. Install it with cargo:

````
cargo install gateway
````

Confirm the install by printing the version:

````
gateway --version
````

## Start the gateway

Start the gateway with one subcommand that names a config file and a profile:

````
gateway serve gateway.toml --profile main
````

The first argument is the path to the config file. The `--profile` flag names the profile to activate. The gateway always starts from one config file and one active profile.

You can supply both values through environment variables instead of command-line arguments. The config path comes from the positional argument or from `PROMPTFORGE_GATEWAY_CONFIG`; the command line wins when both are set. The profile comes from `--profile`, then `PROMPTFORGE_PROFILE`, then the sibling state file the gateway keeps beside the config.

## Check that it is healthy

Once the gateway is serving, probe its health endpoint:

````
curl http://127.0.0.1:8081/health
````

GET /health needs no credentials. It always answers 200 while the gateway is serving.

Every /v1 route requires the shared bearer key from the config file. The address in this request is the `bind` value from the `[server]` section of the config file; `127.0.0.1:8081` is an example bind. A request with a wrong token is rejected with status 401 and error code `unauthorized`:

````
curl -H "Authorization: Bearer wrong-token" http://127.0.0.1:8081/v1/models
````

## Choose what to build

Build-time feature flags decide which capabilities exist in the binary. The flags `local`, `web-search`, and `config-ui` are on by default. The `workshop` flag is opt-in. A headless build without `local` refuses any configuration that declares local models; the refusal happens at startup and again on any profile switch.

## Run it as a service on Linux

On Linux the release archive contains a sample systemd unit. The unit runs the gateway as a service with a fixed config path and profile, and restarts it automatically on failure:

````
ExecStart=/usr/local/bin/gateway serve /etc/promptforge/gateway.toml --profile main
Restart=on-failure
RestartSec=5
````

The gateway holds vendor credentials, so run it as a dedicated unprivileged user. The sample unit does this with `DynamicUser=yes` and keeps state in a systemd-managed state directory (`StateDirectory=promptforge`).

## Watch the logs

Control log verbosity through the standard `RUST_LOG` environment filter. The speech library logs at warn level by default, so it stays quiet unless you ask for more.

Startup failures appear on stderr with the full cause chain: one `error:` line followed by one `caused by:` line per cause. Once the gateway is serving, the log shows the bound address. If you configured port 0, the log reports the real bound port.

## Stop the gateway

Stop the gateway cleanly with Ctrl-C. If the gateway hosts a workshop, the workshop stops first, and then the gateway drains.

