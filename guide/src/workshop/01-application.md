# The Application

This chapter teaches you what the Workshop desktop application is, how to install and start it, and what you see the first time its window opens. Everything else in this guide happens inside this one window, so it is worth a few minutes to understand what the application is made of and how it boots before you touch any feature.

## What the Workshop is

PromptForge Workshop is a desktop application for Windows, macOS, and Linux. You launch one program named Workshop. That program boots a small server inside itself and then opens a single window titled "PromptForge". The window shows the Workshop interface, which the built-in server serves on your own machine. There is no separate web server to install and no files to download before the interface can appear; the interface ships bundled inside the application.

The Workshop talks to a PromptForge gateway. The gateway is the part of the system that supplies the model catalog, the profiles, and the model rounds that power chat. The gateway runs as its own program, separate from the Workshop window: the application's built-in server attaches to a running gateway over HTTP, so closing the window never unloads the gateway or its loaded models. The window opens at 1024 by 768 pixels the first time, and it remembers its size, position, and maximized state across launches.

The application shows the PromptForge program icon in its custom title bar.

## Installing and starting the Workshop

You receive the application as a Windows installer, a macOS disk image, a Debian package, or a Linux AppImage, depending on your platform. On Windows the installer silently includes the webview runtime the application needs, so there is no separate setup step.

To start the application, launch it the way you launch any installed program on your platform. If you work from a source checkout instead, one command builds and starts it:

````
cargo run -p workshop
````

To check which version you have without starting anything, run:

````
promptforge-workshop --version
````

This prints the version and exits. It does not start the server and it does not open a window.

The installed application can also check for updates and update itself. After startup it automatically checks the latest GitHub Release, and it installs only cryptographically verified updates.

You can also run the Workshop's server on its own and use the interface in an ordinary browser. In that mode you open the chat UI at `http://127.0.0.1:7910/`. The browser session works like the desktop window for almost everything; the few differences, such as native window controls and Explorer drag-and-drop, are called out in the chapters that cover them.

## The first launch

The first time you start the Workshop, the application prepares everything it needs before you see a window. Follow what happens:

1. The application looks for its boot configuration.
2. It starts its server inside its own process and waits until the server accepts connections.
3. It waits for the interface to answer a health check, up to 15 seconds.
4. Only then does the window open.

You never see a window before the interface is ready, and the interface never opens against a dead server. If the server does not answer in time, the error message names the health endpoint and how long the application waited. If startup fails for any reason, the application prints the full error chain and exits with a failure code instead of opening a broken window.

Only one instance of the Workshop runs at a time. If you launch it again while it is already running, the existing window comes into focus instead of a second copy opening. When you close the window, the application shuts its built-in server down cleanly and exits; the gateway is a separate program and keeps running. In-flight connections get a 5-second grace window, so a held chat session or a stuck request cannot hang the shutdown. The interface listens on an OS-assigned loopback port, so another program holding a port can never block startup.

The Workshop also keeps working when parts of its environment fail. The interface still loads when the gateway is unreachable, so a gateway outage never prevents the application from opening. If microphone setup fails at startup, you keep working and only voice input stays unavailable. On Windows, if the bridge to Explorer fails to attach, the application keeps running and loses only Explorer drag-and-drop and the microphone grant.

## The gateway configuration

The gateway owns its own boot config, `gateway.toml`, and the Workshop never reads it. On the gateway's first run - when no config exists anywhere it searches - the gateway writes a default `gateway.toml` into `%USERPROFILE%\.promptforge\` and prints a message telling you where it wrote the file. It also creates `profiles\default.toml` beside it, and it never overwrites an existing `profiles\default.toml`. The generated config boots the gateway into the `default` profile.

The generated config is a single editable TOML file with a header that invites edits. Two properties of the generated file are worth knowing:

- The gateway is secured with a freshly generated random bearer key, so no two installs share a key.
- The gateway listens on the loopback address only, on an OS-assigned port. It is not reachable from other machines, and the Workshop learns the port from the connection file the gateway writes.

A `gateway.toml` carried over from an older version may declare a `[workshop]` section. It still parses: the gateway logs a deprecation warning, its `bind` and `open_browser` settings do nothing (the Workshop's server now lives inside the desktop application), and only the `[workshop.stt]` capture tuning still applies.

At run time the gateway also downloads the pinned voice runtime matched to your machine (CUDA on Windows, Metal on Apple Silicon, CPU on the other supported targets), plus the managed `llama-server`. You make no build-time choices for this.

## The Workshop configuration

You configure the Workshop through a TOML file named `workshop.toml`. The application searches three places in order: beside the executable, the current directory, and `~/.promptforge/workshop.toml`. The first file found wins. Every field is optional and the defaults are built in. With no file anywhere, the application keeps its state in `~/.promptforge/` and attaches to the gateway through its connection file. The application never writes the file, and the standalone server's `workbench.toml` fallback does not apply to it.

The keys you are most likely to set:

- `gateway.base_url` points the Workshop at a PromptForge gateway the connection file cannot see, such as one on another machine. When the value is empty, the Workshop attaches to a locally running gateway through its connection file, and with no gateway running, startup fails with an error that names both remedies.
- `gateway.api_key` supplies the bearer key for the gateway API. An empty key sends no `Authorization` header, which is right for a gateway running with authentication disabled.
- `server.bind` is honored only by the standalone `workshop-server` binary. The desktop application owns its listener and always binds `127.0.0.1` on an OS-assigned port.
- `server.state_dir` chooses where the Workshop keeps persistent state. Agent session event logs live under `state_dir/sessions/`, and the per-profile model memory is written there. It defaults to the config file's own directory.
- `agents.path` chooses which directory of `.lua` agent programs is launchable. The default is `agents/` beside the config file. A missing directory offers no agents; that is a state, not an error.

String values support `${VAR}` environment interpolation, so you can keep secrets out of the file. A literal dollar sign is written `$$`. An unset variable interpolates to the empty string instead of failing startup.

The configuration is strict about mistakes, so you find out about problems immediately. A config without a `[gateway]` section fails to load. Unknown keys or sections are a startup error, such as a leftover `[voice]` section from an older version. Error messages name the offending file, and a malformed `${...}` interpolation gives a clear error. A browser launch failure, by contrast, is only logged as a warning; it never stops the server.

## Working with your operating system

The Workshop is a desktop citizen, not just a web page in a frame.

You can drag files from your operating system and drop them into the application to attach them. You can open native file and folder picker dialogs from the Workshop. When you click a link to an external website, it opens in your system browser while the Workshop window stays on its own page. Links between pages served by the Workshop itself load inside the application window.

One protection is worth understanding early: a link to any other local server, even one on the same port spelled `localhost` or `[::1]`, opens in the system browser. No other program on your machine gets the application's desktop features.

## Safety and limits

The Workshop is built so that only you, on your own machine, can reach it.

The window loads its interface only from the local machine, never from a remote address. The Workshop refuses any request a browser marks as coming from another website, and it only answers requests addressed to a loopback host. Requests that change things must declare a JSON body. The live socket that carries chat only upgrades for the Workshop's own loopback origin or a native client.

Nothing hangs forever. A stalled request is answered with a timeout error instead of freezing: ordinary routes give up after 10 seconds, and routes that relay a call to the gateway allow up to 35 seconds so a stalled gateway surfaces as a meaningful failure. Live socket sessions are never cut off by a request deadline. A gateway that is down or wedged fails fast in the interface: connections give up after 5 seconds and ordinary requests after 30 seconds.

Startup also cleans up after previous runs. Leftover temporary files in the state directory are swept away on boot, so a crash during a previous save never leaves residue that affects the next launch.

You now know what the application is, how it starts, and what it connects to. The next chapter opens the window and walks through its regions.

