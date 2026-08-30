// The tab bar's profile switcher [Adapted: workshop]: a dropdown button
// showing the active profile, opening a menu of every profile with the
// active one checked - the workshop Model menu's Profiles section
// pattern (radio rows, check on active, rows disabled while a switch is
// in flight). Selecting another profile posts the switch and drives its
// SSE stages into the full-screen apply overlay.

import { Check, ChevronDown, createElement as lucideElement } from "lucide";

import { UnauthorizedError } from "../services/gateway-api";
import type { GatewayApi } from "../services/gateway-api";
import type { ApplyOverlay } from "./apply-overlay";
import type { ToastStack } from "./toast";

/** Construction dependencies for the switcher. */
export interface ProfileSwitcherDeps {
  /** The admin API client. */
  api: GatewayApi;
  /** The full-screen stage overlay shown while a switch runs. */
  overlay: ApplyOverlay;
  /** Error surfacing for failed switches and profile listings. */
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
  let switching = false;

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

  const openMenu = async () => {
    button.setAttribute("aria-expanded", "true");
    menu.hidden = false;
    document.addEventListener("click", onDocumentClick);
    let profiles: string[];
    try {
      profiles = await deps.api.getProfiles();
    } catch (error) {
      closeMenu();
      if (!(error instanceof UnauthorizedError)) {
        deps.toasts.show("Could not list profiles", "error");
      }
      return;
    }
    renderRows(profiles);
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
      row.setAttribute("aria-checked", name === active ? "true" : "false");
      row.disabled = switching;
      const mark = document.createElement("span");
      mark.className = "menu-check";
      if (name === active) {
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

  const select = async (name: string) => {
    if (switching) {
      return;
    }
    if (name === active) {
      closeMenu();
      button.focus();
      return;
    }
    switching = true;
    setRowsDisabled(true);
    // The pending mark: the chosen row is announced busy while in flight.
    const target = [...menu.querySelectorAll<HTMLButtonElement>(".menu-item")].find(
      (row) => row.textContent === name,
    );
    target?.classList.add("is-pending");
    target?.setAttribute("aria-busy", "true");
    deps.overlay.open(`Switching to ${name}`);

    let result;
    try {
      result = await deps.api.switchProfile(name, (stage) => deps.overlay.beginStage(stage));
    } catch (error) {
      switching = false;
      if (error instanceof UnauthorizedError) {
        // The unauthorized path is tearing the shell down around us.
        deps.overlay.finish();
        return;
      }
      deps.overlay.fail("Gateway unreachable");
      deps.toasts.show("Gateway unreachable", "error");
      closeMenu();
      button.focus();
      return;
    }
    switching = false;
    if (result.status === "ready") {
      deps.overlay.finish();
      active = result.profile;
      label.textContent = active;
    } else {
      deps.overlay.fail(result.message);
      deps.toasts.show(result.message, "error");
    }
    closeMenu();
    button.focus();
  };

  button.addEventListener("click", () => {
    if (menu.hidden) {
      void openMenu();
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

  return {
    element,
    setActiveProfile(name: string): void {
      active = name;
      label.textContent = name;
    },
  };
}
