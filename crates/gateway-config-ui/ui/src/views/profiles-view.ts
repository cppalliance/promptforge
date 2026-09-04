// Profile checklist editor. Each profile is one ordered subset of the
// global model catalog, edited through APG-style rearrangeable listboxes.

import {
  ArrowLeft,
  ArrowRight,
  CircleX,
  Info,
  TriangleAlert,
  createElement as lucideElement,
} from "lucide";

import { confirmDialog } from "../components/confirm-modal";
import { createDropdownControl } from "../components/dropdown-control";
import type { ToastStack } from "shared-ui/toast";
import type {
  ConfigStore,
  ModelEntry,
  ProfileEntry,
} from "../services/config-store";

/** Fraction of a local dominion budget where headroom becomes risky. */
export const VRAM_WARN_FRACTION = 0.8;

/** Construction dependencies for the profile checklist view. */
export interface ProfilesViewDeps {
  /** Pending catalog and profile state. */
  store: ConfigStore;
  /** Save and validation outcomes. */
  toasts: ToastStack;
}

/** The mounted Profiles view. */
export interface ProfilesView {
  /** Renders the profile editor into `main`. */
  mount(main: HTMLElement): () => void;
}

type Pane = "available" | "chosen";

/** Validates profile identifiers before they cross the API boundary. */
export function profileNameError(name: string): string | null {
  if (name.length === 0) {
    return "Enter a profile name.";
  }
  if (/[/\\\0]/.test(name)) {
    return "A profile name must be a single name without path separators.";
  }
  if (name === "." || name === "..") {
    return "A profile name must not be a traversal component.";
  }
  return null;
}

/** Builds the Profiles view. */
export function createProfilesView(deps: ProfilesViewDeps): ProfilesView {
  const { store, toasts } = deps;
  let main: HTMLElement | null = null;
  let viewRoot: HTMLElement | null = null;
  let selectedProfile = "";
  let availableQuery = "";
  let chosenQuery = "";
  let saving = false;
  const selected = {
    available: new Set<string>(),
    chosen: new Set<string>(),
  } satisfies Record<Pane, Set<string>>;
  const activeIndex = {
    available: 0,
    chosen: 0,
  } satisfies Record<Pane, number>;
  const typeahead = {
    available: "",
    chosen: "",
  } satisfies Record<Pane, string>;
  const typeaheadTimer: Record<Pane, ReturnType<typeof setTimeout> | null> = {
    available: null,
    chosen: null,
  };
  const live = document.createElement("p");
  live.className = "visually-hidden";
  live.setAttribute("aria-live", "polite");
  live.setAttribute("aria-atomic", "true");

  const announce = (message: string): void => {
    live.textContent = "";
    queueMicrotask(() => {
      if (live.isConnected) {
        live.textContent = message;
      }
    });
  };

  store.subscribe(() => {
    if (main?.isConnected && viewRoot?.isConnected) {
      render();
    }
  });

  const currentProfile = (): ProfileEntry | null => {
    const profiles = store.profiles();
    if (!profiles.some((profile) => profile.name === selectedProfile)) {
      selectedProfile =
        profiles.find((profile) => profile.name === store.pendingActiveProfile())?.name ??
        profiles[0]?.name ??
        "";
    }
    return profiles.find((profile) => profile.name === selectedProfile) ?? null;
  };

  const unfilteredEntriesFor = (pane: Pane): ModelEntry[] => {
    const profile = currentProfile();
    if (!profile) {
      return [];
    }
    const chosen = new Set(profile.models);
    return store
      .models()
      .filter((entry) => !entry.draft && chosen.has(entry.name) === (pane === "chosen"));
  };

  const entriesFor = (pane: Pane): ModelEntry[] => {
    const query = (pane === "available" ? availableQuery : chosenQuery).trim().toLowerCase();
    return unfilteredEntriesFor(pane)
      .filter((entry) => query === "" || entry.name.toLowerCase().includes(query));
  };

  const commitMove = async (pane: Pane, names: readonly string[]): Promise<void> => {
    const profile = currentProfile();
    if (!profile || saving || names.length === 0) {
      return;
    }
    const moved = new Set(names);
    const next =
      pane === "available"
        ? [...profile.models, ...names]
        : profile.models.filter((name) => !moved.has(name));
    const destination: Pane = pane === "available" ? "chosen" : "available";
    const focusName = names[0] ?? "";
    let movedSuccessfully = false;
    saving = true;
    try {
      await store.saveProfile(profile.name, next);
      selected.available.clear();
      selected.chosen.clear();
      const destinationEntries = entriesFor(destination);
      activeIndex[destination] = Math.max(
        0,
        destinationEntries.findIndex((entry) => entry.name === focusName),
      );
      movedSuccessfully = true;
    } catch (error) {
      toasts.show(error instanceof Error ? error.message : "The profile could not be saved", "error");
    } finally {
      saving = false;
      render();
    }
    if (movedSuccessfully) {
      const destinationLabel = destination === "chosen" ? "Chosen" : "Available";
      announce(
        `${names.length} model${names.length === 1 ? "" : "s"} moved to ${destinationLabel}.`,
      );
      const movedOption = main?.querySelector<HTMLElement>(
        `#profile-${destination}-${safeId(focusName)}`,
      );
      if (movedOption) {
        movedOption.focus();
      } else {
        main
          ?.querySelector<HTMLInputElement>(`#profile-${destination}-search`)
          ?.focus();
      }
    }
  };

  const stageActive = async (): Promise<void> => {
    const profile = currentProfile();
    if (!profile || saving || profile.name === store.pendingActiveProfile()) {
      return;
    }
    saving = true;
    let message: string | null = null;
    try {
      await store.stageActiveProfile(profile.name);
      message = `${profile.name} will become active on Apply.`;
      toasts.show(message, "success");
    } catch (error) {
      toasts.show(error instanceof Error ? error.message : "The active profile could not be staged", "error");
    } finally {
      saving = false;
      render();
    }
    if (message !== null) {
      announce(message);
    }
  };

  const deleteProfile = async (): Promise<void> => {
    const profile = currentProfile();
    if (!main || !profile || profile.name === store.pendingActiveProfile()) {
      return;
    }
    const yes = await confirmDialog(main, {
      title: `Delete ${profile.name}?`,
      body: `Remove the profile ${profile.name} from the pending configuration.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!yes) {
      return;
    }
    try {
      await store.deleteProfile(profile.name);
      selectedProfile = "";
      toasts.show(`Deleted profile ${profile.name}`, "success");
    } catch (error) {
      toasts.show(error instanceof Error ? error.message : "The profile could not be deleted", "error");
    }
  };

  const render = (): void => {
    if (!main) {
      return;
    }
    const root = document.createElement("div");
    root.className = "profiles-view";
    const title = document.createElement("h1");
    title.className = "view-title";
    title.textContent = "Profiles";
    root.append(title);

    if (!store.loaded) {
      const skeleton = document.createElement("div");
      skeleton.className = "skeleton-row";
      skeleton.setAttribute("aria-hidden", "true");
      root.append(skeleton);
    } else if (store.loadError) {
      const failure = document.createElement("p");
      failure.className = "banner banner-danger";
      failure.textContent = store.loadError;
      root.append(failure);
    } else {
      const split = document.createElement("div");
      split.className = "profiles-split";
      split.append(profileList(), editor());
      root.append(split);
    }
    root.append(live);
    viewRoot = root;
    main.replaceChildren(root);
  };

  const profileList = (): HTMLElement => {
    const pane = document.createElement("section");
    pane.className = "profile-list-pane";
    const heading = document.createElement("h2");
    heading.className = "section-heading";
    heading.textContent = "Profiles";
    const create = document.createElement("button");
    create.type = "button";
    create.className = "button button-primary new-profile";
    create.textContent = "New Profile";
    create.addEventListener("click", openCreateDialog);
    const list = document.createElement("ul");
    list.className = "profile-list";
    for (const profile of store.profiles()) {
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.className = "profile-select";
      button.setAttribute("aria-pressed", String(profile.name === currentProfile()?.name));
      const name = document.createElement("span");
      name.className = "profile-name";
      name.textContent = profile.name;
      const count = document.createElement("span");
      count.className = "pill";
      count.textContent = String(profile.models.length);
      button.append(name, count);
      if (profile.name === store.activeProfile) {
        const active = document.createElement("span");
        active.className = "pill pill-accent";
        active.textContent = "Active";
        button.append(active);
      } else if (profile.name === store.pendingActiveProfile()) {
        const pending = document.createElement("span");
        pending.className = "pill pill-accent";
        pending.textContent = "Pending";
        button.append(pending);
      }
      button.addEventListener("click", () => {
        selectedProfile = profile.name;
        selected.available.clear();
        selected.chosen.clear();
        render();
      });
      item.append(button);
      list.append(item);
    }
    pane.append(heading, create, list);
    return pane;
  };

  const editor = (): HTMLElement => {
    const pane = document.createElement("section");
    pane.className = "profile-summary-pane";
    const profile = currentProfile();
    if (!profile) {
      const empty = document.createElement("p");
      empty.className = "view-empty";
      empty.textContent = "Create a profile to choose models.";
      pane.append(empty);
      return pane;
    }
    const header = document.createElement("header");
    header.className = "profile-editor-header";
    const heading = document.createElement("h2");
    heading.className = "profile-summary-title";
    heading.textContent = profile.name;
    const actions = document.createElement("div");
    actions.className = "detail-actions";
    const setActive = document.createElement("button");
    setActive.type = "button";
    setActive.className = "button button-primary set-active";
    setActive.textContent =
      profile.name === store.pendingActiveProfile() ? "Selected for Apply" : "Set Active";
    setActive.disabled = saving || profile.name === store.pendingActiveProfile();
    setActive.addEventListener("click", () => void stageActive());
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "button button-danger profile-delete";
    remove.textContent = "Delete";
    remove.disabled = saving || profile.name === store.pendingActiveProfile();
    remove.title =
      profile.name === store.pendingActiveProfile()
        ? "Choose another active profile before deleting this one."
        : "";
    remove.addEventListener("click", () => void deleteProfile());
    actions.append(setActive, remove);
    header.append(heading, actions);

    const shuttle = document.createElement("div");
    shuttle.className = "profile-shuttle";
    shuttle.append(
      listPane("available", "Available"),
      moveControls(),
      listPane("chosen", "Chosen"),
    );
    pane.append(header, vramSummary(profile), shuttle);
    return pane;
  };

  const listPane = (pane: Pane, labelText: string): HTMLElement => {
    const wrap = document.createElement("section");
    wrap.className = `shuttle-pane shuttle-${pane}`;
    const head = document.createElement("div");
    head.className = "shuttle-head";
    const heading = document.createElement("h3");
    heading.className = "profiles-heading";
    heading.textContent = labelText;
    const allEntries = entriesFor(pane);
    const totalEntries = unfilteredEntriesFor(pane);
    const count = document.createElement("span");
    count.className = "shuttle-count";
    count.textContent =
      `${selected[pane].size} selected, ${allEntries.length} of ${totalEntries.length} shown`;
    head.append(heading, count);

    const searchId = `profile-${pane}-search`;
    const searchLabel = document.createElement("label");
    searchLabel.className = "visually-hidden";
    searchLabel.htmlFor = searchId;
    searchLabel.textContent = `Search ${labelText}`;
    const search = document.createElement("input");
    search.type = "search";
    search.id = searchId;
    search.className = "input shuttle-search";
    search.placeholder = `Search ${labelText.toLowerCase()}`;
    search.value = pane === "available" ? availableQuery : chosenQuery;
    search.addEventListener("input", () => {
      if (pane === "available") {
        availableQuery = search.value;
      } else {
        chosenQuery = search.value;
      }
      activeIndex[pane] = 0;
      render();
      main?.querySelector<HTMLInputElement>(`#${searchId}`)?.focus();
    });

    const list = document.createElement("ul");
    list.className = "shuttle-list";
    list.id = `profile-${pane}-list`;
    list.setAttribute("role", "listbox");
    list.setAttribute("aria-label", labelText);
    list.setAttribute("aria-multiselectable", "true");
    const entries = entriesFor(pane);
    activeIndex[pane] = Math.min(activeIndex[pane], Math.max(0, entries.length - 1));
    entries.forEach((entry, index) => list.append(option(entry, pane, index, entries)));
    if (entries.length === 0) {
      const empty = document.createElement("li");
      empty.className = "view-empty shuttle-empty";
      empty.textContent = "No matching models.";
      list.append(empty);
    }
    wrap.append(head, searchLabel, search, list);
    return wrap;
  };

  const option = (
    entry: ModelEntry,
    pane: Pane,
    index: number,
    entries: ModelEntry[],
  ): HTMLElement => {
    const item = document.createElement("li");
    item.className = "shuttle-option";
    item.id = `profile-${pane}-${safeId(entry.name)}`;
    item.setAttribute("role", "option");
    item.setAttribute("aria-selected", String(selected[pane].has(entry.name)));
    item.tabIndex = index === activeIndex[pane] ? 0 : -1;
    const name = document.createElement("span");
    name.className = "model-name";
    name.textContent = entry.name;
    item.append(name, kindBadge(entry));
    const toggle = (): void => {
      if (selected[pane].has(entry.name)) {
        selected[pane].delete(entry.name);
      } else {
        selected[pane].add(entry.name);
      }
      render();
      main
        ?.querySelector<HTMLElement>(`#profile-${pane}-${safeId(entry.name)}`)
        ?.focus();
    };
    item.addEventListener("click", toggle);
    item.addEventListener("focus", () => {
      activeIndex[pane] = index;
    });
    item.addEventListener("keydown", (event) => {
      if (event.key === " ") {
        event.preventDefault();
        toggle();
        return;
      }
      const destination = keyDestination(event.key, index, entries.length);
      if (destination !== null) {
        event.preventDefault();
        activeIndex[pane] = destination;
        render();
        main
          ?.querySelector<HTMLElement>(
            `#profile-${pane}-${safeId(entries[destination]?.name ?? "")}`,
          )
          ?.focus();
        return;
      }
      if (event.key.length === 1 && /\S/.test(event.key)) {
        typeahead[pane] += event.key.toLowerCase();
        const match = entries.findIndex((candidate) =>
          candidate.name.toLowerCase().startsWith(typeahead[pane]),
        );
        if (match >= 0) {
          activeIndex[pane] = match;
          render();
          main
            ?.querySelector<HTMLElement>(
              `#profile-${pane}-${safeId(entries[match]?.name ?? "")}`,
            )
            ?.focus();
        }
        const timer = typeaheadTimer[pane];
        if (timer !== null) {
          clearTimeout(timer);
        }
        typeaheadTimer[pane] = setTimeout(() => {
          typeahead[pane] = "";
          typeaheadTimer[pane] = null;
        }, 500);
      }
    });
    return item;
  };

  const moveControls = (): HTMLElement => {
    const controls = document.createElement("div");
    controls.className = "shuttle-controls";
    const choose = document.createElement("button");
    choose.type = "button";
    choose.className = "button button-outline shuttle-choose";
    choose.disabled = saving || selected.available.size === 0;
    choose.setAttribute("aria-label", "Move selection to Chosen");
    choose.append(
      lucideElement(ArrowRight, { "aria-hidden": "true", width: 16, height: 16 }),
    );
    choose.addEventListener("click", () =>
      void commitMove("available", [...selected.available]),
    );
    const unchoose = document.createElement("button");
    unchoose.type = "button";
    unchoose.className = "button button-outline shuttle-unchoose";
    unchoose.disabled = saving || selected.chosen.size === 0;
    unchoose.setAttribute("aria-label", "Move selection to Available");
    unchoose.append(
      lucideElement(ArrowLeft, { "aria-hidden": "true", width: 16, height: 16 }),
    );
    unchoose.addEventListener("click", () => void commitMove("chosen", [...selected.chosen]));
    controls.append(choose, unchoose);
    return controls;
  };

  const vramSummary = (profile: ProfileEntry): HTMLElement => {
    const chosen = new Set(profile.models);
    const local = store
      .models()
      .filter((entry) => chosen.has(entry.name) && entry.kind !== "remote");
    const known = local.filter((entry) => typeof entry.data["vram_gb"] === "number");
    const total = known.reduce((sum, entry) => sum + Number(entry.data["vram_gb"]), 0);
    const unknown = local.filter((entry) => typeof entry.data["vram_gb"] !== "number");
    const section = document.createElement("section");
    section.className = "vram-summary";
    const heading = document.createElement("h3");
    heading.className = "profiles-heading";
    heading.textContent = "Estimated VRAM";
    const info = document.createElement("button");
    info.type = "button";
    info.className = "vram-info";
    info.setAttribute("aria-label", "Explain the VRAM estimate");
    info.title =
      "The estimate sums declared model weights. KV cache grows with context length, and runtime plus driver overhead also consume VRAM, so 20% headroom is recommended.";
    info.append(lucideElement(Info, { "aria-hidden": "true", width: 14, height: 14 }));
    const totalLine = document.createElement("p");
    totalLine.className = "vram-total";
    totalLine.textContent = `${formatGb(total)} GB estimated`;
    totalLine.append(info);
    section.append(heading, totalLine);
    if (unknown.length > 0) {
      const unknownLine = document.createElement("p");
      unknownLine.className = "vram-unknown";
      unknownLine.textContent = `Unknown: ${unknown.map((entry) => entry.name).join(", ")}`;
      section.append(unknownLine);
    }
    for (const dominion of store.dominions().filter(
      (entry) => entry.kind === "local" && entry.vramGb !== null,
    )) {
      const contributors = local.filter((entry) => entry.data["dominion"] === dominion.id);
      const unknownContributors = contributors.filter(
        (entry) => typeof entry.data["vram_gb"] !== "number",
      );
      const sum = contributors.reduce(
        (value, entry) =>
          value + (typeof entry.data["vram_gb"] === "number" ? entry.data["vram_gb"] : 0),
        0,
      );
      const budget = dominion.vramGb ?? 0;
      const fraction = budget > 0 ? sum / budget : Number.POSITIVE_INFINITY;
      const row = document.createElement("p");
      row.className = "vram-budget";
      row.dataset["state"] =
        fraction > 1 ? "over" : fraction >= VRAM_WARN_FRACTION ? "warning" : "normal";
      if (fraction > 1) {
        row.append(lucideElement(CircleX, { "aria-hidden": "true", width: 14, height: 14 }));
      } else if (fraction >= VRAM_WARN_FRACTION) {
        row.append(
          lucideElement(TriangleAlert, { "aria-hidden": "true", width: 14, height: 14 }),
        );
      }
      row.append(
        document.createTextNode(
          `${dominion.id}: ${formatGb(sum)}${
            unknownContributors.length === 0
              ? ""
              : ` + ${unknownContributors.length} unknown`
          } / ${formatGb(budget)} GB`,
        ),
      );
      if (unknownContributors.length > 0) {
        row.title = `Unknown VRAM: ${unknownContributors.map((entry) => entry.name).join(", ")}`;
      }
      section.append(row);
    }
    return section;
  };

  const openCreateDialog = (): void => {
    if (!main) {
      return;
    }
    const profiles = store.profiles();
    const overlay = document.createElement("div");
    overlay.className = "overlay dialog-overlay";
    const card = document.createElement("section");
    card.className = "modal new-profile-dialog";
    card.setAttribute("role", "dialog");
    card.setAttribute("aria-modal", "true");
    const opener = document.activeElement;
    const close = (): void => {
      overlay.remove();
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
    const heading = document.createElement("h2");
    heading.id = "new-profile-title";
    heading.textContent = "New Profile";
    card.setAttribute("aria-labelledby", heading.id);
    const form = document.createElement("form");
    const label = document.createElement("label");
    label.htmlFor = "new-profile-name";
    label.textContent = "Name";
    const input = document.createElement("input");
    input.id = "new-profile-name";
    input.className = "input";
    input.autocomplete = "off";
    const error = document.createElement("p");
    error.id = "new-profile-error";
    error.className = "field-error";
    error.setAttribute("aria-live", "polite");
    input.setAttribute("aria-describedby", error.id);
    let mode = "empty";
    let copyFrom = profiles[0]?.name ?? "";
    const modes = document.createElement("fieldset");
    modes.className = "start-from";
    const legend = document.createElement("legend");
    legend.textContent = "Start from";
    modes.append(legend);
    const copyControl = createDropdownControl({
      id: "new-profile-copy-from",
      options: profiles.map((profile) => ({ value: profile.name, label: profile.name })),
      value: copyFrom,
      onChange: (value) => {
        copyFrom = value;
      },
    });
    copyControl.trigger.setAttribute("aria-label", "Profile to copy");
    copyControl.setDisabled(true);
    modes.append(
      radio("empty", "Empty", true, () => {
        mode = "empty";
        copyControl.setDisabled(true);
      }),
      radio("copy", "Copy of", false, () => {
        mode = "copy";
        copyControl.setDisabled(false);
      }, copyControl.element),
    );
    const actions = document.createElement("div");
    actions.className = "modal-actions";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "button button-outline";
    cancel.textContent = "Cancel";
    cancel.addEventListener("click", close);
    const submit = document.createElement("button");
    submit.type = "submit";
    submit.className = "button button-primary";
    submit.textContent = "Create";
    actions.append(cancel, submit);
    form.append(label, input, error, modes, actions);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const name = input.value.trim();
      const refusal =
        profileNameError(name) ??
        (profiles.some((profile) => profile.name === name) ? `Profile ${name} already exists.` : null);
      if (refusal !== null) {
        error.textContent = refusal;
        input.setAttribute("aria-invalid", "true");
        return;
      }
      error.textContent = "";
      input.removeAttribute("aria-invalid");
      submit.disabled = true;
      void store
        .createProfile(name, mode === "copy" ? copyFrom : null)
        .then(() => {
          selectedProfile = name;
          overlay.remove();
          render();
          [...(main?.querySelectorAll<HTMLButtonElement>(".profile-select") ?? [])]
            .find((button) => button.querySelector(".profile-name")?.textContent === name)
            ?.focus();
          toasts.show(`Created profile ${name}`, "success");
        })
        .catch((requestError: unknown) => {
          submit.disabled = false;
          error.textContent =
            requestError instanceof Error ? requestError.message : "The profile could not be created";
        });
    });
    card.append(heading, form);
    card.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        close();
        return;
      }
      if (event.key !== "Tab") {
        return;
      }
      const controls = [...card.querySelectorAll<HTMLElement>("input, button")].filter(
        (element) =>
          !(element as HTMLInputElement).disabled &&
          !element.hidden &&
          element.closest("[hidden]") === null,
      );
      const first = controls[0];
      const last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    });
    overlay.addEventListener("click", (event) => {
      if (event.target === overlay) {
        close();
      }
    });
    overlay.append(card);
    main.append(overlay);
    input.focus();
  };

  return {
    mount(target: HTMLElement): () => void {
      main = target;
      viewRoot = null;
      render();
      return () => {
        for (const pane of ["available", "chosen"] as const) {
          const timer = typeaheadTimer[pane];
          if (timer !== null) {
            clearTimeout(timer);
            typeaheadTimer[pane] = null;
          }
        }
        main = null;
        viewRoot = null;
      };
    },
  };
}

function radio(
  value: string,
  labelText: string,
  checked: boolean,
  onChange: () => void,
  extra?: HTMLElement,
): HTMLElement {
  const row = document.createElement("div");
  row.className = "radio-row";
  const input = document.createElement("input");
  input.type = "radio";
  input.name = "start-from";
  input.id = `start-from-${value}`;
  input.value = value;
  input.checked = checked;
  input.addEventListener("change", onChange);
  const label = document.createElement("label");
  label.htmlFor = input.id;
  label.textContent = labelText;
  row.append(input, label);
  if (extra) {
    row.append(extra);
  }
  return row;
}

function kindBadge(entry: ModelEntry): HTMLElement {
  const badge = document.createElement("span");
  badge.className = "pill profile-kind";
  badge.dataset["kind"] = entry.kind;
  badge.textContent = entry.kind === "local" ? "Cpu" : entry.kind === "remote" ? "Cloud" : "Mic";
  return badge;
}

function keyDestination(key: string, current: number, length: number): number | null {
  if (length === 0) {
    return null;
  }
  if (key === "ArrowDown") {
    return Math.min(length - 1, current + 1);
  }
  if (key === "ArrowUp") {
    return Math.max(0, current - 1);
  }
  if (key === "Home") {
    return 0;
  }
  return key === "End" ? length - 1 : null;
}

function safeId(value: string): string {
  return encodeURIComponent(value).replaceAll("%", "_");
}

function formatGb(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}
