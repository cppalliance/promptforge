// The tab bar's profile switcher [Adapted: workshop]: a dropdown button
// showing the selected profile, opening a menu of "No profile" followed
// by every profile, with the persisted selection checked. Picking a row
// posts `POST /admin/switch-profile`; the running profile never changes
// in place, so a pick that differs from it raises the restart banner.

import { Check, ChevronDown, createElement as lucideElement } from "lucide";

import type { ConfigStore } from "../services/config-store";
import type { ToastStack } from "shared-ui/toast";

/** The row label for the null selection. */
const NO_PROFILE_LABEL = "No profile";

/** Construction dependencies for the switcher. */
export interface ProfileSwitcherDeps {
  /** Pending configuration state and the switch path. */
  store: ConfigStore;
  /** Error surfacing for a refused switch. */
  toasts: ToastStack;
  /** Runs when the gateway reports the selection needs a restart to run. */
  onRestartRequired: () => void;
}

/** The mounted switcher and its live-update handle. */
export interface ProfileSwitcher {
  /** The wrapper element holding the trigger button and its menu. */
  element: HTMLElement;
  /** Sets the running profile name (empty for none) the label compares against. */
  setActiveProfile(name: string): void;
}

/** Builds the profile switcher. */
export function createProfileSwitcher(deps: ProfileSwitcherDeps): ProfileSwitcher {
  let running = "";
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
  prefix.textContent = "Selected profile:";
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
  menu.setAttribute("aria-label", "Select profile");
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

  const selectedName = (): string | null => deps.store.selectedProfile();

  const labelFor = (name: string | null): string => name ?? NO_PROFILE_LABEL;

  const paintLabel = (): void => {
    const selected = selectedName();
    const differs = (selected ?? "") !== running;
    label.textContent = labelFor(selected);
    label.classList.toggle("is-pending", differs);
    button.title = differs
      ? `${labelFor(selected)} is selected; restart the gateway to run it`
      : "";
  };

  const openMenu = (): void => {
    button.setAttribute("aria-expanded", "true");
    menu.hidden = false;
    document.addEventListener("click", onDocumentClick);
    renderRows(deps.store.profiles().map((profile) => profile.name));
    // The menu pattern: focus lands on the checked row so the arrow
    // keys work from the moment the menu opens.
    const landing =
      menu.querySelector<HTMLButtonElement>("[aria-checked='true']") ??
      menu.querySelector<HTMLButtonElement>(".menu-item");
    landing?.focus();
  };

  const renderRows = (profiles: string[]) => {
    const selected = selectedName();
    // "No profile" leads whenever at least one profile is defined; with
    // none defined there is nothing to select away from.
    const choices: Array<string | null> = profiles.length > 0 ? [null, ...profiles] : [];
    const rows = choices.map((name) => {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "menu-item";
      row.setAttribute("role", "menuitemradio");
      row.setAttribute("aria-checked", name === selected ? "true" : "false");
      row.dataset["profile"] = name ?? "";
      row.disabled = switching;
      const mark = document.createElement("span");
      mark.className = "menu-check";
      if (name === selected) {
        mark.append(lucideElement(Check, { "aria-hidden": "true", width: 14, height: 14 }));
      }
      const text = document.createElement("span");
      text.textContent = labelFor(name);
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

  const select = async (name: string | null): Promise<void> => {
    if (switching) {
      return;
    }
    if (name === selectedName()) {
      closeMenu();
      button.focus();
      return;
    }
    switching = true;
    setRowsDisabled(true);
    let restartRequired: boolean;
    try {
      // A dataset comparison, not an attribute selector: a profile name
      // may contain `"` or `]`, which would make a built selector throw.
      const target = [...menu.querySelectorAll<HTMLButtonElement>(".menu-item")].find(
        (row) => row.dataset["profile"] === (name ?? ""),
      );
      target?.classList.add("is-pending");
      target?.setAttribute("aria-busy", "true");
      restartRequired = await deps.store.selectProfile(name);
    } catch (error) {
      deps.toasts.show(
        error instanceof Error ? error.message : "The profile could not be selected",
        "error",
      );
      closeMenu();
      button.focus();
      switching = false;
      return;
    }
    switching = false;
    paintLabel();
    if (restartRequired) {
      deps.onRestartRequired();
      deps.toasts.show(`${labelFor(name)} selected - restart the gateway to run it`, "success");
    } else {
      deps.toasts.show(`${labelFor(name)} selected`, "success");
    }
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
    // An empty name means "no profile runs" only once status has loaded;
    // before that the shell's own status probe owns the value.
    if (deps.store.loaded && deps.store.loadError === null) {
      running = deps.store.activeProfile;
    }
    paintLabel();
  });

  return {
    element,
    setActiveProfile(name: string): void {
      running = name;
      paintLabel();
    },
  };
}
