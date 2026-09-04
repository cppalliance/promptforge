# The Application

This chapter teaches you what the Workshop desktop application is, how to install and start it, and what you see the first time its window opens. Everything else in this guide happens inside this one window, so it is worth a few minutes to understand what the application is made of and how it boots before you touch any feature.

## What the Workshop is

PromptForge Workshop is a desktop application for Windows, macOS, and Linux. You launch one program named Workshop. That program boots a small server inside itself and then opens a single window titled "PromptForge". The window shows the Workshop interface, which the built-in server serves on your own machine. There is no separate web server to install and no files to download before the interface can appear; the interface ships bundled inside the application.

The Workshop talks to a PromptForge gateway. The gateway is the part of the system that supplies the model catalog, the profiles, and the model rounds that power chat. In the standard setup the gateway runs in the same process as the Workshop, so starting the application starts everything you need. The window opens at 1024 by 768 pixels the first time, and it remembers its size, position, and maximized state across launches.

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

This prints the version and exits. It does not boot the gateway and it does not open a window.

The installed application can also check for updates and update itself. After startup it automatically checks the latest GitHub Release, and it installs only cryptographically verified updates.

You can also run the Workshop's server on its own and use the interface in an ordinary browser. In that mode you open the chat UI at `http://127.0.0.1:7910/`. The browser session works like the desktop window for almost everything; the few differences, such as native window controls and Explorer drag-and-drop, are called out in the chapters that cover them.

## The first launch

The first time you start the Workshop, the application prepares everything it needs before you see a window. Follow what happens:

1. The application looks for its boot configuration.
2. It starts its server inside its own process and waits until the server accepts connections.
3. It waits for the interface to answer a health check, up to 15 seconds.
4. Only then does the window open.

You never see a window before the interface is ready, and the interface never opens against a dead server. If the server does not answer in time, the error message names the health endpoint and how long the application waited. If startup fails for any reason, the application prints the full error chain and exits with a failure code instead of opening a broken window.

Only one instance of the Workshop runs at a time. If you launch it again while it is already running, the existing window comes into focus instead of a second copy opening. When you close the window, the whole application shuts down cleanly, gateway included. In-flight connections get a 5-second grace window, so a held chat session or a stuck request cannot hang the shutdown, and the port is released so you can restart immediately on the same address. If the configured port is already taken by another program, startup fails with a clear error rather than silently serving nothing.

The Workshop also keeps working when parts of its environment fail. The interface still loads when the gateway is unreachable, so a gateway outage never prevents the application from opening. If microphone setup fails at startup, you keep working and only voice input stays unavailable. On Windows, if the bridge to Explorer fails to attach, the application keeps running and loses only Explorer drag-and-drop and the microphone grant.

## The boot configuration

The application reads a boot config named `gateway.toml`. When it starts, it searches three places in order:

1. Beside the executable.
2. The current directory.
3. `%USERPROFILE%\.promptforge\gateway.toml`.

The first file found wins. This means you can drop a `gateway.toml` beside the executable or in the current directory to override the profile copy.

On the very first run, when no config exists anywhere, the application does not fail. It writes a default `gateway.toml` into `%USERPROFILE%\.promptforge\` and prints a message telling you where it wrote the file. It also creates `profiles\default.toml` beside it, and it never overwrites an existing `profiles\default.toml`. The application always boots into the `default` profile.

The generated config is a single editable TOML file with a header that invites edits. Two properties of the generated file are worth knowing:

- The gateway is secured with a freshly generated random bearer key, so no two installs share a key.
- The gateway listens on the loopback address only, bound to `127.0.0.1:8081`. It is not reachable from other machines. The Workshop UI itself is hosted on a second loopback-only listener at `127.0.0.1:7910`.

If you want the application to also open a browser tab alongside the desktop window, set `workshop.open_browser` in the boot config. The generated config leaves it off. If the boot config has no `[workshop]` section, the application tells you to add one to `gateway.toml`.

At run time the application also downloads the pinned voice runtime matched to your machine (CUDA on Windows, Metal on Apple Silicon, CPU on the other supported targets), plus the managed `llama-server`. You make no build-time choices for this.

## The Workshop configuration

Beyond the boot config, you configure the Workshop through a TOML file named `workshop.toml`. The application reads it by default when no other path is given. Every field is optional and the defaults are built in. The zero-config path you saw for `gateway.toml` applies here too: on first run the application writes a default `workshop.toml` into `~/.promptforge/` and loads that. If you have an older `workbench.toml` file, the application picks it up automatically when no `workshop.toml` is present.

The keys you are most likely to set:

- `gateway.base_url` points the Workshop at a PromptForge gateway. When the value is empty, the Workshop falls back to the built-in default `http://127.0.0.1:8081` instead of failing.
- `gateway.api_key` supplies the bearer key for the gateway API. An empty key sends no `Authorization` header, which is right for a gateway running with authentication disabled.
- `server.bind` changes the address the Workshop binds to. The default is `127.0.0.1:7910`.
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

