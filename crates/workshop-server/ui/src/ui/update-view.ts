// Update banner and installation overlay. Native I/O and state stay in the
// service; this view only translates snapshots into DOM. Transient update
// notifications ride the shared toast stack (shared-ui/toast); the banner
// keeps the actionable "available" state and the overlay the install
// progress (the shared inline progress bar).

import "./update-view.css";

import { createProgressBar } from "shared-ui/progress";
import type { ToastStack } from "shared-ui/toast";

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
  private notifiedPhase: UpdateSnapshot["phase"] | null = null;

  constructor(
    private readonly service: UpdateService,
    private readonly toasts: ToastStack,
  ) {
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
    this.notify(snapshot);
    this.renderBanner(snapshot);
    this.renderScreen(snapshot);
  }

  /** Toasts the phase transitions a user must notice without watching. */
  private notify(snapshot: UpdateSnapshot): void {
    if (snapshot.phase === this.notifiedPhase) {
      return;
    }
    this.notifiedPhase = snapshot.phase;
    if (snapshot.phase === "available") {
      this.toasts.show(`PromptForge ${snapshot.version} is available`, "info");
    } else if (snapshot.phase === "error") {
      this.toasts.show(`Update failed: ${snapshot.error}`, "error");
    }
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
    const progress = createProgressBar("Update download progress");
    progress.setFraction(progressValue === null ? null : progressValue / 100);
    const details = document.createElement("details");
    const summary = document.createElement("summary");
    summary.textContent = "Update log";
    const log = document.createElement("pre");
    log.textContent = snapshot.log.join("\n");
    details.append(summary, log);
    panel.append(title, status, progress.element, details);
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
