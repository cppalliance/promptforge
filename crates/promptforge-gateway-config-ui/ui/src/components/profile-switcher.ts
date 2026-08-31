// The tab bar's profile switcher [Adapted: workshop]: a dropdown button
// showing the active profile, opening a menu of every profile with the
// pending one checked. Selecting another profile stages
// `active_profile`; Apply performs the runtime switch atomically with
// every other configuration edit.

import { Check, ChevronDown, createElement as lucideElement } from "lucide";

import type { ConfigStore } from "../services/config-store";
import type { ToastStack } from "./toast";

/** Construction dependencies for the switcher. */
export interface ProfileSwitcherDeps {
  /** Pending configuration state and write path. */
  store: ConfigStore;
  /** Error surfacing for failed staging. */
  toasts: ToastStack;
}

/** The mounted switcher and its live-update handle. */
export interface ProfileSwitcher {
  /** The wrapper element holding the trigger button and its menu. */
  element: HTMLElement;
  /** Sets the active profile name shown on the trigger. */
  setActiveProfile(name: string): void;
}

/** Builds the profile switcher. */
export function createProfileSwitcher(deps: ProfileSwitcherDeps): ProfileSwitcher {
  let active = "";
  let staging = false;

  const element = document.createElement("div");
  element.className = "profile-switcher";

  const button = document.createElement("button");
  button.type = "button";
  button.className = "select select-sm";
  button.setAttribute("aria-haspopup", "menu");
  button.setAttribute("aria-expanded", "false");
  const prefix = document.createElement("span");
  prefix.className = "visually-hidden";
  prefix.textContent = "Active profile:";
  const label = document.createElement("span");
  label.textContent = "\u2026";
  button.append(
    prefix,
    label,
    lucideElement(ChevronDown, { "aria-hidden": "true", width: 14, height: 14 }),
  );

  const menu = document.createElement("div");
  menu.className = "menu";
  menu.setAttribute("role", "menu");
  menu.setAttribute("aria-label", "Switch profile");
  menu.hidden = true;

  element.append(button, menu);

  const onDocumentClick = (event: Event) => {
    if (!element.contains(event.target as Node)) {
      closeMenu();
    }
  };

  const closeMenu = () => {
    menu.hidden = true;
    button.setAttribute("aria-expanded", "false");
    document.removeEventListener("click", onDocumentClick);
  };

  const pendingName = (): string => deps.store.pendingActiveProfile() || active;

  const paintLabel = (): void => {
    const pending = pendingName();
    label.textContent = pending;
    label.classList.toggle("is-pending", pending !== active);
    button.title = pending !== active ? `${pending} will become active on Apply` : "";
  };

  const openMenu = (): void => {
    button.setAttribute("aria-expanded", "true");
    menu.hidden = false;
    document.addEventListener("click", onDocumentClick);
    renderRows(deps.store.profiles().map((profile) => profile.name));
    // The menu pattern: focus lands on the active row so the arrow
    // keys work from the moment the menu opens.
    const landing =
      menu.querySelector<HTMLButtonElement>("[aria-checked='true']") ??
      menu.querySelector<HTMLButtonElement>(".menu-item");
    landing?.focus();
  };

  const renderRows = (profiles: string[]) => {
    const rows = profiles.map((name) => {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "menu-item";
      row.setAttribute("role", "menuitemradio");
      row.setAttribute("aria-checked", name === pendingName() ? "true" : "false");
      row.disabled = staging;
      const mark = document.createElement("span");
      mark.className = "menu-check";
      if (name === pendingName()) {
        mark.append(lucideElement(Check, { "aria-hidden": "true", width: 14, height: 14 }));
      }
      const text = document.createElement("span");
      text.textContent = name;
      row.append(mark, text);
      row.addEventListener("click", () => void select(name));
      return row;
    });
    menu.replaceChildren(...rows);
  };

  const setRowsDisabled = (disabled: boolean) => {
    for (const row of menu.querySelectorAll<HTMLButtonElement>(".menu-item")) {
      row.disabled = disabled;
    }
  };

  const select = async (name: string): Promise<void> => {
    if (staging) {
      return;
    }
    if (name === pendingName()) {
      closeMenu();
      button.focus();
      return;
    }
    staging = true;
    setRowsDisabled(true);
    const target = [...menu.querySelectorAll<HTMLButtonElement>(".menu-item")].find(
      (row) => row.textContent === name,
    );
    target?.classList.add("is-pending");
    target?.setAttribute("aria-busy", "true");
    try {
      await deps.store.stageActiveProfile(name);
    } catch (error) {
      deps.toasts.show(error instanceof Error ? error.message : "The profile could not be staged", "error");
      closeMenu();
      button.focus();
      staging = false;
      return;
    }
    staging = false;
    paintLabel();
    deps.toasts.show(`${name} will become active on Apply`, "success");
    closeMenu();
    button.focus();
  };

  button.addEventListener("click", () => {
    if (menu.hidden) {
      openMenu();
    } else {
      closeMenu();
    }
  });

  // On the wrapper, not the menu, so Escape and the arrows also work
  // while focus still sits on the trigger button.
  element.addEventListener("keydown", (event) => {
    if (menu.hidden) {
      return;
    }
    if (event.key === "Escape") {
      closeMenu();
      button.focus();
      return;
    }
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") {
      return;
    }
    event.preventDefault();
    const rows = [...menu.querySelectorAll<HTMLButtonElement>(".menu-item")];
    if (rows.length === 0) {
      return;
    }
    const current = rows.indexOf(document.activeElement as HTMLButtonElement);
    const step = event.key === "ArrowDown" ? 1 : -1;
    const next = (current + step + rows.length) % rows.length;
    rows[next]?.focus();
  });

  deps.store.subscribe(() => {
    if (deps.store.activeProfile !== "") {
      active = deps.store.activeProfile;
    }
    paintLabel();
  });

  return {
    element,
    setActiveProfile(name: string): void {
      active = name;
      paintLabel();
    },
  };
}
