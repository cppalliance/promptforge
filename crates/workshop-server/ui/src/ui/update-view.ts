// Update banner and installation overlay. Native I/O and state stay in the
// service; this view only translates snapshots into DOM.

import "./update-view.css";

import { Disposable, toDisposable } from "../base/lifecycle";
import { UpdateService, type UpdateSnapshot } from "../services/update-service";

function percentage(snapshot: UpdateSnapshot): number | null {
  if (snapshot.total === null || snapshot.total <= 0) {
    return null;
  }
  return Math.min(100, Math.round((snapshot.downloaded / snapshot.total) * 100));
}

export class UpdateView extends Disposable {
  private readonly banner = document.createElement("aside");
  private readonly overlay = document.createElement("div");
  private installing = false;

  constructor(private readonly service: UpdateService) {
    super();
    this.banner.className = "update-banner";
    this.overlay.className = "update-screen";
    document.body.append(this.banner, this.overlay);
    this._register(toDisposable(() => this.banner.remove()));
    this._register(toDisposable(() => this.overlay.remove()));
    this._register(service.onDidChange((snapshot) => this.render(snapshot)));
    this.render(service.snapshot);
  }

  private render(snapshot: UpdateSnapshot): void {
    this.renderBanner(snapshot);
    this.renderScreen(snapshot);
  }

  private renderBanner(snapshot: UpdateSnapshot): void {
    this.banner.replaceChildren();
    this.banner.hidden = snapshot.phase !== "available" || this.installing;
    if (this.banner.hidden) {
      return;
    }
    const title = document.createElement("strong");
    title.textContent = `PromptForge ${snapshot.version} is available`;
    const notes = document.createElement("p");
    notes.textContent = snapshot.notes || "A new desktop release is ready.";
    const actions = document.createElement("div");
    actions.className = "update-banner__actions";
    const later = document.createElement("button");
    later.type = "button";
    later.textContent = "Remind me later";
    later.addEventListener("click", () => this.service.remindLater());
    const install = document.createElement("button");
    install.type = "button";
    install.className = "update-banner__primary";
    install.textContent = "Update now";
    install.addEventListener("click", () => {
      this.installing = true;
      this.render(this.service.snapshot);
      void this.service.install();
    });
    actions.append(later, install);
    this.banner.append(title, notes, actions);
  }

  private renderScreen(snapshot: UpdateSnapshot): void {
    const active =
      this.installing &&
      ["downloading", "installing", "restarting", "error"].includes(snapshot.phase);
    this.overlay.hidden = !active;
    this.overlay.replaceChildren();
    if (!active) {
      return;
    }
    const panel = document.createElement("section");
    panel.className = "update-screen__panel";
    panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-modal", "true");
    panel.setAttribute("aria-labelledby", "update-screen-title");
    const title = document.createElement("h2");
    title.id = "update-screen-title";
    title.textContent = `Updating to PromptForge ${snapshot.version}`;
    const status = document.createElement("p");
    const progressValue = percentage(snapshot);
    if (snapshot.phase === "downloading") {
      status.textContent =
        progressValue === null ? "Downloading update..." : `Downloading update... ${progressValue}%`;
    } else if (snapshot.phase === "installing") {
      status.textContent = "Installing update...";
    } else if (snapshot.phase === "restarting") {
      status.textContent = "Restarting PromptForge...";
    } else {
      status.textContent = `Update failed: ${snapshot.error}`;
    }
    const progress = document.createElement("progress");
    progress.max = 100;
    if (progressValue !== null) {
      progress.value = progressValue;
    }
    const details = document.createElement("details");
    const summary = document.createElement("summary");
    summary.textContent = "Update log";
    const log = document.createElement("pre");
    log.textContent = snapshot.log.join("\n");
    details.append(summary, log);
    panel.append(title, status, progress, details);
    if (snapshot.phase === "error") {
      const close = document.createElement("button");
      close.type = "button";
      close.textContent = "Close";
      close.addEventListener("click", () => {
        this.installing = false;
        this.render(this.service.snapshot);
      });
      panel.appendChild(close);
    }
    this.overlay.appendChild(panel);
  }
}
