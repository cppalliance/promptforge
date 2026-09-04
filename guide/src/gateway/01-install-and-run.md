# Install and Run

This chapter teaches you to get the gateway running on your machine: how to install it, how to start it with a config file and a profile, and how to confirm it is healthy. You do these things every time you bring the gateway up, so they are worth learning well.

## Install the binary

The gateway is a single binary named `promptforge-gateway`. It serves an OpenAI-shaped inference API. Install it with cargo:

````
cargo install gateway
````

Confirm the install by printing the version:

````
promptforge-gateway --version
````

## Start the gateway

Start the gateway with one subcommand that names a config file and a profile:

````
promptforge-gateway serve gateway.toml --profile main
````

The first argument is the path to the config file. The `--profile` flag names the profile to activate. The gateway always starts from one config file and one active profile.

You can supply both values through environment variables instead of command-line arguments. The config path comes from the positional argument or from `PROMPTFORGE_GATEWAY_CONFIG`; the command line wins when both are set. The profile comes from `--profile`, then `PROMPTFORGE_PROFILE`, then the sibling state file the gateway keeps beside the config.

You can also start the gateway with no config file at all. When no `gateway.toml` exists beside the executable, in the working directory, or in the user profile's `.promptforge` directory, the first run writes a default config there - loopback-only on an OS-assigned port, with a fresh random bearer key and `trust_loopback = true` so callers on the same machine need no key - and boots from it. The generated file notes the caveat beside that line: on a shared machine any other OS account can then use the gateway, and `trust_loopback = false` requires the key from everyone. The generated config selects a profile named `default`, so a bare first boot needs no flags.

## The system tray

On a desktop system the gateway's face is the system tray. The icon shows the gateway's state, and its menu carries a status line, a Workshop item that launches the Workshop application when the installer laid it beside the gateway, a Settings item that opens the configuration UI in your browser, a Launch at Login toggle, and Quit. A gateway started at login never opens a browser or a window.

For servers and CI, `--no-tray` keeps the plain headless loop. In a tray-less environment, `--print-url` prints the Settings URL to stdout once the gateway is bound. `--browser` opens the Settings page in your default browser once bound; the installer uses it on a Gateway-only install's first run. Launching `promptforge-gateway` while one is already running never starts a second copy: it opens the running gateway's Settings page instead.

After every successful bind the gateway writes a connection file (`gateway.json` in the run directory under the state directory) carrying its port, bearer key, and process id. PromptForge components read that file to attach to the running gateway instead of starting a second one, and a clean shutdown removes it.

## Check that it is healthy

Once the gateway is serving, probe its health endpoint:

````
curl http://127.0.0.1:8081/health
````

GET /health needs no credentials. It always answers 200 while the gateway is serving.

Every /v1 route is authenticated with the shared bearer key from the config file. The address in this request is the `bind` value from the `[server]` section of the config file; `127.0.0.1:8081` is an example bind. A request with a wrong token is rejected with status 401 and error code `unauthorized`, from any peer:

````
curl -H "Authorization: Bearer wrong-token" http://127.0.0.1:8081/v1/models
````

From the gateway's own machine you can leave the key out entirely. With the default `trust_loopback = true`, a loopback request that presents no credential is admitted:

````
curl http://127.0.0.1:8081/v1/models
````

This convenience has one cost: on a shared machine, any other OS account can use the gateway the same way, including reading upstream API keys from the admin config surface. Set `trust_loopback = false` in `[server]` to require the key from every caller. The configuration chapter covers the rule in full.

## Choose what to build

Build-time feature flags decide which capabilities exist in the binary. The flags `local`, `web-search`, `stt`, and `config-ui` are on by default. A headless build without `local` refuses any configuration that declares local models; the refusal happens at startup and again on any profile switch.

## Run it as a service on Linux

On Linux the release archive contains a sample systemd unit. The unit runs the gateway as a service with a fixed config path and profile, and restarts it automatically on failure:

````
ExecStart=/usr/local/bin/promptforge-gateway serve /etc/promptforge/gateway.toml --profile main
Restart=on-failure
RestartSec=5
````

The gateway holds vendor credentials, so run it as a dedicated unprivileged user. The sample unit does this with `DynamicUser=yes` and keeps state in a systemd-managed state directory (`StateDirectory=promptforge`).

## Watch the logs

Control log verbosity through the standard `RUST_LOG` environment filter. The speech library logs at warn level by default, so it stays quiet unless you ask for more.

Startup failures appear on stderr with the full cause chain: one `error:` line followed by one `caused by:` line per cause. Once the gateway is serving, the log shows the bound address. If you configured port 0, the log reports the real bound port.

## Stop the gateway

From the tray, choose Quit. From a script or another PromptForge component, send an authenticated POST to the `/shutdown` route with the bearer key; it answers 202 and then the server goes down. Under `--no-tray`, Ctrl-C stops the gateway cleanly. Every path drains in-flight requests before the exit.

