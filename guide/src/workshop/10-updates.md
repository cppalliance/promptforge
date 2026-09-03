# Updates and Configuration

You can operate the whole application: the window, the panels, the menus, the status bar, models, chat, voice, the workspace, and the editor. This final chapter teaches you to keep the Workshop current and tuned: the update flow, the About dialog, and the embedded Gateway Config panel.

## Keeping the Workshop up to date

The installed application automatically checks the latest GitHub Release shortly after startup and installs only cryptographically verified updates. Downloaded updates are verified against a pinned public key before installation, so tampered updates are rejected. The automatic check runs on the desktop application only, and update checks give up after 30 seconds rather than hanging. On Windows, updates install passively, applying with minimal interruption to your session.

Platform notes:

- On Linux the update flow is available only when running as an AppImage. Package-managed installations show the update flow as unsupported and never contact the update endpoint.
- In a plain browser session the update flow stays inert.
- Nightly builds do not produce updater artifacts, so a nightly install does not receive automatic in-app updates.

When an update is available, you see a banner floating at the bottom-right corner of the window, above the status bar. The banner shows the new version number and a one-line summary of the release notes. You have two choices:

- Click "Remind me later" to dismiss the banner and bring the prompt back later.
- Click "Update now" to start the update immediately.

While an update downloads, installs, or restarts, a full-screen modal overlay takes over the window. You watch download progress as a percentage and a progress bar, with bytes received against the total size. After the download finishes, the application installs the update and restarts itself.

When an update download or install fails, you see the failure reason and can dismiss the overlay with a Close button to return to the application. You can expand an "Update log" section in the overlay to read the raw log lines produced during the update. When the application is already up to date, the update state reports that no update is available. When an update check fails, you see an error message.

## The About dialog

Open Help > About PromptForge to see the About dialog. It names the product, the application version, and the license, shown as "License: BSL-1.0". A development build shows the version "dev" instead of a release number.

The About dialog is also where you trigger an update check manually. The update button reflects the state:

- "Desktop updates unavailable" in a browser.
- "Updates are managed by your package manager" on package-managed installs.
- "Checking for updates..." while a check runs.
- "Show update <version>" when an update is ready.
- "Retry update check" after a failed check.

The About dialog traps keyboard focus: Tab and Shift+Tab cycle between its buttons and never leave the modal. You can dismiss it with the Escape key or the Close button, and focus returns to the element that opened it. Only one About dialog can be open at a time.

## The Gateway Config panel

You can view and change gateway configuration without leaving the Workshop, in the Gateway Config panel. The panel opens in the main zone through the application's Gateway Config command, titled "Gateway Config". Opening it a second time focuses the existing panel instead of opening a duplicate, and you can close it from its tab's close action.

The panel embeds the gateway's configuration web interface, served same-origin through the Workshop at the `/gateway/config/` route in panel mode. It opens in the dark theme on the local gateway view. From the panel you can:

- View the gateway's current configuration.
- Edit and save gateway configuration and environment values.
- Apply or revert pending configuration changes, and see whether the configuration has unsaved edits or changes waiting to be applied.
- Search and browse Hugging Face models.
- View gateway status, system information, model information, chat templates, environment, and orphaned files.
- View the downloaded model cache and delete a cached model to free disk space.
- Trigger the gateway's reveal action.

Panel actions are announced on the Workshop status bar: "Gateway configuration applied", "Gateway configuration changes reverted", and "Gateway download started". Long-running panel operations such as cache downloads and profile switches can stream for minutes without being cut off by a timeout. When the gateway is unreachable, the panel reports the failure instead of hanging.

You never handle the gateway access key. The Workshop server attaches the bearer key on the server side of every forwarded panel request. Neither the Workshop page nor the embedded config panel ever sees it, and the key is never written to logs. The panel's API requests go through an allowlisted proxy; anything outside the configuration surface is refused, including chat completions, progress subscriptions, health checks, and direct cache uploads. Deleting a cached model is allowed only by its 64-character lowercase hex digest. Requests with malformed or absolute targets are refused locally with a forbidden status before anything leaves the application. The panel is reachable only from your own machine, never from the local network, and the embedded configuration interface runs in a restricted sandbox limited to running scripts within the same origin.

## Reskinning the interface

If you build the Workshop from source, you can reskin the entire interface by editing CSS custom properties in the `:root` block of `ui/style.css`. Every color, spacing step, radius, font, scrollbar metric, and the status bar's LED and progress effect is a custom property there. To reskin without editing the shipped stylesheet, add a `<link>` after `/style.css` in `ui/index.html` and redeclare any variable on `:root`. Later declarations win the cascade. Focus on menus and controls is shown through state backgrounds, opacity, or underlines, never through outline rings or focus boxes.

You have completed the tour. You can install and start the Workshop, read its window and status bar, pick models and switch profiles, converse with an agent by keyboard or voice, grant folders, edit files, and keep the application current and configured.

