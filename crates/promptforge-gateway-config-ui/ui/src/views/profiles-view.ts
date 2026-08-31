// The Profiles view [INVENTED as a whole]: no researched UI manages
// profiles; the individual elements are copied patterns. Left: the
// profile list from GET /admin/profiles - active row with a green dot
// and an Active pill [Adapted: llama-swap], a per-row kebab menu
// [Unsloth] row hover actions with Set Active (the switch-profile SSE
// flow into the shared apply overlay) and Delete (confirm; the server
// answers 409 for the active profile), and a [New Profile] dialog
// [Adapted: Unsloth] with Empty / Copy of / Include modes. Right: the
// active profile's summary card - model counts, allowlist chips, and
// the include chain editor. Its rows come from the payload's top-level
// `include` array (the leaf's own line, verbatim and ordered - shadow
// preferred), so order is authoritative and a fully-overridden parent
// still lists; exists/missing indicators flag a deleted parent (the
// accepted-risk surface), with up/down reorder buttons, remove, and
// Add Include with autocomplete over the existing .toml files plus
// create-new. Saving the chain PUTs the leaf shadow with an explicit
// `include` array; server cycle/depth
// validation errors surface on the banner. [Edit] drills into
// #/profiles/include/{path}, where a simplified generic editor (one
// labeled JSON textarea per top-level section of the file's
// provenance-derived pending content) saves via PUT /admin/include.

import {
  ArrowDown,
  ArrowUp,
  EllipsisVertical,
  X,
  createElement as lucideElement,
} from "lucide";

import type { ApplyOverlay } from "../components/apply-overlay";
import { confirmDialog } from "../components/confirm-modal";
import { createDropdownControl } from "../components/dropdown-control";
import type { ToastStack } from "../components/toast";
import type { ChainFile, ConfigStore, EntryData } from "../services/config-store";
import { GatewayHttpError, UnauthorizedError } from "../services/gateway-api";
import type { CreateProfileBody, GatewayApi } from "../services/gateway-api";

/** Construction dependencies for the view. */
export interface ProfilesViewDeps {
  /** The config store: pending view, chain derivation, payload builders. */
  store: ConfigStore;
  /** The admin API: profiles list, create/delete, switch, include saves. */
  api: GatewayApi;
  /** Outcome surfacing. */
  toasts: ToastStack;
  /** The full-screen SSE stage overlay the switch flow drives. */
  overlay: ApplyOverlay;
  /** Fired after a successful Set Active switch, with the new name. */
  onSwitched: (name: string) => void;
}

/** The mounted view handle the router calls. */
export interface ProfilesView {
  /** Renders the view; `includePath` set renders the drill-in editor. */
  mount(main: HTMLElement, includePath?: string): void;
}

/**
 * Validates a profile name against the server's trust-boundary rule:
 * exactly one path component - no separators, no `.`/`..` traversal,
 * no NUL, not empty. Returns the refusal sentence, or null when valid.
 */
export function profileNameError(name: string): string | null {
  if (name.length === 0) {
    return "Enter a profile name.";
  }
  if (/[/\\\0]/.test(name)) {
    return "A profile name must be a single file name without path separators.";
  }
  if (name === "." || name === "..") {
    return "A profile name must not be a traversal component.";
  }
  return null;
}

/** Strips a trailing `.toml` from an include-file name. */
function stem(name: string): string {
  return name.replace(/\.toml$/, "");
}

/** The drill-in route for one chain file. */
function drillHref(path: string): string {
  return `#/profiles/include/${encodeURIComponent(path)}`;
}

/** Builds the Profiles view (state survives route re-mounts). */
export function createProfilesView(deps: ProfilesViewDeps): ProfilesView {
  const { store, api, toasts, overlay } = deps;

  let main: HTMLElement | null = null;
  /** The last-rendered root; a re-render is legal only while it owns `main`. */
  let viewRoot: HTMLElement | null = null;
  /** The profile names, null until the first listing answers. */
  let profiles: string[] | null = null;
  let listError: string | null = null;
  /** The summary card's profile; defaults to the active one. */
  let selected: string | null = null;
  let switching = false;
  /** The user-modified chain; null renders the derived chain. */
  let chainEdit: ChainFile[] | null = null;
  /** The chain save's server validation error (cycle/depth). */
  let chainError: string | null = null;
  /** The drill-in target, from the route; null renders the list. */
  let drillPath: string | null = null;
  /** Drill-in textarea contents by top-level key, seeded per target. */
  const drillEdits = new Map<string, string>();
  let drillError: string | null = null;

  store.subscribe(() => {
    if (main?.isConnected && viewRoot?.isConnected) {
      render();
    }
  });

  /** Sets the route hash (and pokes the router, for jsdom). */
  const navigate = (hash: string): void => {
    const win = document.defaultView;
    if (!win) {
      return;
    }
    win.location.hash = hash;
    win.dispatchEvent(new win.Event("hashchange"));
  };

  const refreshProfiles = async (): Promise<void> => {
    try {
      profiles = await api.getProfiles();
      listError = null;
    } catch (error) {
      if (error instanceof UnauthorizedError) {
        return;
      }
      listError = error instanceof Error ? error.message : String(error);
    }
    render();
  };

  // ----- the switch flow (Set Active), mirroring the tab-bar switcher ------

  const setActive = async (name: string): Promise<void> => {
    if (switching || name === store.activeProfile) {
      return;
    }
    switching = true;
    overlay.open(`Switching to ${name}`);
    let result;
    try {
      result = await api.switchProfile(name, (stage) => overlay.beginStage(stage));
    } catch (error) {
      switching = false;
      if (error instanceof UnauthorizedError) {
        // The unauthorized path is tearing the shell down around us.
        overlay.finish();
        return;
      }
      overlay.fail("Gateway unreachable");
      toasts.show("Gateway unreachable", "error");
      return;
    }
    switching = false;
    if (result.status === "ready") {
      overlay.finish();
      selected = result.profile;
      chainEdit = null;
      chainError = null;
      deps.onSwitched(result.profile);
    } else {
      overlay.fail(result.message);
      toasts.show(result.message, "error");
    }
    render();
  };

  const deleteProfile = async (name: string): Promise<void> => {
    if (!main) {
      return;
    }
    const yes = await confirmDialog(main, {
      title: "Delete profile?",
      body: `This deletes ${name}.toml from the profiles directory. A profile that includes it will fail to load until the include is removed.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!yes) {
      return;
    }
    try {
      await api.deleteProfile(name);
    } catch (error) {
      if (error instanceof UnauthorizedError) {
        return;
      }
      // The active profile's 409 refusal lands here with the server's
      // own sentence.
      toasts.show(error instanceof Error ? error.message : "The delete failed", "error");
      return;
    }
    toasts.show(`Deleted ${name}.toml`, "success");
    if (selected === name) {
      selected = null;
    }
    await refreshProfiles();
  };

  // ----- the New Profile dialog ---------------------------------------------

  const openNewProfileDialog = (): void => {
    if (!main || profiles === null) {
      return;
    }
    newProfileDialog(main, api, profiles, (name) => {
      toasts.show(`Created ${name}.toml`, "success");
      selected = name;
      void refreshProfiles();
    });
  };

  // ----- the include chain editor -------------------------------------------

  /** The rows the editor shows: the user's edit, else the derived chain. */
  const chainRows = (): ChainFile[] => chainEdit ?? store.includeChain();

  /** Copies the derived chain into the editable state, once. */
  const editableChain = (): ChainFile[] => {
    if (chainEdit === null) {
      chainEdit = store.includeChain().map((row) => ({ ...row }));
    }
    return chainEdit;
  };

  /**
   * Whether a chain file is missing from disk: a plain profile-dir file
   * absent from the listing. This is the accepted-risk surface: a
   * deleted parent renders as Missing. Files outside the profiles dir
   * (the boot file) and subdirectory entries are not listable and never
   * flagged.
   */
  const isMissing = (row: ChainFile): boolean =>
    !row.outside &&
    !/[/\\]/.test(row.path) &&
    profiles !== null &&
    !profiles.includes(stem(row.path));

  const moveRow = (index: number, delta: number): void => {
    const rows = editableChain();
    const target = index + delta;
    if (target < 0 || target >= rows.length) {
      return;
    }
    const [row] = rows.splice(index, 1);
    if (row) {
      rows.splice(target, 0, row);
    }
    render();
  };

  const removeRow = (index: number): void => {
    editableChain().splice(index, 1);
    render();
  };

  const addInclude = async (raw: string): Promise<void> => {
    const name = raw.trim().endsWith(".toml") ? raw.trim() : `${raw.trim()}.toml`;
    const nameError = profileNameError(stem(name));
    if (stem(name).length === 0 || nameError !== null) {
      chainError = nameError ?? "Enter a file name.";
      render();
      return;
    }
    if (chainRows().some((row) => row.path === name)) {
      toasts.show(`${name} is already in the chain`, "info");
      return;
    }
    // Create-new: an include naming a file that does not exist would
    // fail the save's merged-chain validation, so the file is created
    // (empty) through the profile-create route first.
    if (profiles !== null && !profiles.includes(stem(name))) {
      try {
        await api.createProfile(stem(name), { mode: "empty" });
      } catch (error) {
        if (error instanceof UnauthorizedError) {
          return;
        }
        chainError = error instanceof Error ? error.message : "The file could not be created";
        render();
        return;
      }
      toasts.show(`Created ${name}`, "success");
      await refreshProfiles();
    }
    editableChain().push({ path: name, base: name, outside: false });
    chainError = null;
    render();
  };

  const saveChain = async (): Promise<void> => {
    if (chainEdit === null) {
      return;
    }
    const payload = store.buildConfigPayload();
    payload["include"] = chainEdit.map((row) => row.path);
    try {
      await store.savePayload(payload);
    } catch (error) {
      if (error instanceof UnauthorizedError) {
        return;
      }
      // Cycle/depth refusals from the merged-chain validation land here.
      chainError = error instanceof Error ? error.message : "The save failed";
      render();
      return;
    }
    chainError = null;
    toasts.show("Include chain saved to the profile shadow", "success");
    render();
  };

  // ----- the drill-in editor -------------------------------------------------

  const saveDrill = async (row: ChainFile): Promise<void> => {
    const body: EntryData = {};
    for (const [key, text] of drillEdits) {
      try {
        body[key] = JSON.parse(text);
      } catch (error) {
        drillError = `The ${key} section is not valid JSON: ${
          error instanceof Error ? error.message : String(error)
        }`;
        render();
        return;
      }
    }
    try {
      await api.putInclude(row.path, body);
    } catch (error) {
      if (error instanceof UnauthorizedError) {
        return;
      }
      drillError = error instanceof Error ? error.message : "The save failed";
      render();
      return;
    }
    drillError = null;
    toasts.show(`Saved to the ${row.base} shadow`, "success");
    // The include shadow changes the merged pending view; reload it.
    void store.load();
  };

  // ----- rendering ------------------------------------------------------------

  const render = (): void => {
    if (!main) {
      return;
    }
    viewRoot = drillPath === null ? renderList() : renderDrillIn(drillPath);
  };

  const renderList = (): HTMLElement => {
    const title = document.createElement("h1");
    title.className = "view-title";
    title.textContent = "Profiles";

    const root = document.createElement("div");
    root.className = "profiles-view";
    root.append(title);

    const failure = store.loadError ?? listError;
    if (failure !== null) {
      root.append(errorBanner(`The profile data failed to load: ${failure}`, () => {
        void store.load();
        void refreshProfiles();
      }));
    }

    const split = document.createElement("div");
    split.className = "profiles-split";
    split.append(listPane(), summaryPane());
    root.append(split);
    main?.replaceChildren(root);
    return root;
  };

  const listPane = (): HTMLElement => {
    const pane = document.createElement("section");
    pane.className = "profile-list-pane";
    pane.setAttribute("aria-label", "Profiles");

    const create = document.createElement("button");
    create.type = "button";
    create.className = "button button-primary new-profile";
    create.textContent = "New Profile";
    create.disabled = profiles === null;
    create.addEventListener("click", openNewProfileDialog);
    pane.append(create);

    if (profiles === null) {
      const skeleton = document.createElement("div");
      skeleton.className = "skeleton-row";
      skeleton.setAttribute("aria-hidden", "true");
      pane.append(skeleton);
      return pane;
    }

    const list = document.createElement("ul");
    list.className = "profile-list";
    for (const name of profiles) {
      list.append(profileRow(name));
    }
    pane.append(list);
    return pane;
  };

  const profileRow = (name: string): HTMLElement => {
    const active = name === store.activeProfile;
    const row = document.createElement("li");
    row.className = "profile-row";

    const select = document.createElement("button");
    select.type = "button";
    select.className = "profile-select";
    select.setAttribute("aria-pressed", name === (selected ?? store.activeProfile) ? "true" : "false");
    const dot = document.createElement("span");
    dot.className = active ? "status-dot is-ok" : "status-dot";
    dot.setAttribute("aria-hidden", "true");
    const label = document.createElement("span");
    label.className = "profile-name";
    label.textContent = name;
    select.append(dot, label);
    if (active) {
      const pill = document.createElement("span");
      pill.className = "pill pill-accent active-pill";
      pill.textContent = "Active";
      select.append(pill);
    }
    select.addEventListener("click", () => {
      selected = name;
      render();
    });

    row.append(select, kebabMenu(name, active));
    return row;
  };

  /** The per-row kebab [Unsloth] row hover actions: Set Active, Delete. */
  const kebabMenu = (name: string, active: boolean): HTMLElement => {
    const wrap = document.createElement("div");
    wrap.className = "profile-kebab";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "field-reset kebab-button";
    button.setAttribute("aria-haspopup", "menu");
    button.setAttribute("aria-expanded", "false");
    button.setAttribute("aria-label", `Actions for ${name}`);
    button.append(lucideElement(EllipsisVertical, { "aria-hidden": "true", width: 16, height: 16 }));

    const menu = document.createElement("div");
    menu.className = "menu kebab-menu";
    menu.setAttribute("role", "menu");
    menu.setAttribute("aria-label", `Actions for ${name}`);
    menu.hidden = true;

    const onDocumentClick = (event: Event) => {
      if (!wrap.contains(event.target as Node)) {
        close();
      }
    };
    const close = () => {
      menu.hidden = true;
      button.setAttribute("aria-expanded", "false");
      document.removeEventListener("click", onDocumentClick);
    };
    const open = () => {
      menu.hidden = false;
      button.setAttribute("aria-expanded", "true");
      document.addEventListener("click", onDocumentClick);
      // role="menu" imposes the keyboard contract: focus moves into
      // the menu on open, arrows walk it, Escape returns to the
      // trigger (the profile-switcher precedent).
      const first = [...menu.querySelectorAll<HTMLButtonElement>(".menu-item")].find(
        (entry) => !entry.disabled,
      );
      first?.focus();
    };

    const item = (labelText: string, disabled: boolean, action: () => void): HTMLElement => {
      const entry = document.createElement("button");
      entry.type = "button";
      entry.className = "menu-item";
      entry.setAttribute("role", "menuitem");
      entry.textContent = labelText;
      entry.disabled = disabled;
      entry.addEventListener("click", () => {
        close();
        action();
      });
      return entry;
    };
    menu.append(
      item("Set Active", active || switching, () => void setActive(name)),
      item("Delete\u2026", false, () => void deleteProfile(name)),
    );

    button.addEventListener("click", () => (menu.hidden ? open() : close()));
    wrap.addEventListener("keydown", (event) => {
      if (menu.hidden) {
        return;
      }
      if (event.key === "Escape") {
        close();
        button.focus();
        return;
      }
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const items = [...menu.querySelectorAll<HTMLButtonElement>(".menu-item")].filter(
          (entry) => !entry.disabled,
        );
        const at = items.findIndex((entry) => entry === document.activeElement);
        const step = event.key === "ArrowDown" ? 1 : -1;
        // Focus outside the items (at === -1) enters at an end.
        const next = at === -1 ? (step === 1 ? 0 : items.length - 1) : (at + step + items.length) % items.length;
        items[next]?.focus();
      }
    });
    wrap.append(button, menu);
    return wrap;
  };

  const summaryPane = (): HTMLElement => {
    const pane = document.createElement("section");
    pane.className = "profile-summary-pane";
    const name = selected ?? store.activeProfile;
    if (!name) {
      const empty = document.createElement("p");
      empty.className = "view-empty";
      empty.textContent = "Select a profile to see its summary.";
      pane.append(empty);
      return pane;
    }
    pane.setAttribute("aria-label", `Profile ${name}`);

    const heading = document.createElement("h2");
    heading.className = "profile-summary-title";
    heading.textContent = name;
    pane.append(heading);

    if (name !== store.activeProfile) {
      const info = document.createElement("p");
      info.className = "field-help";
      info.textContent =
        profiles !== null && profiles.includes(name)
          ? `${name}.toml exists in the profiles directory.`
          : `${name}.toml is not in the profiles directory.`;
      const note = document.createElement("p");
      note.className = "field-help profile-inactive-note";
      // The gateway resolves only the active profile's chain, so model
      // counts, the allowlist, and the include chain exist for it alone.
      note.textContent =
        "Model counts, the allowlist, and the include chain are shown for the active profile only. Set this profile active to see and edit them.";
      pane.append(info, note);
      return pane;
    }

    const activePill = document.createElement("span");
    activePill.className = "pill pill-accent active-pill";
    activePill.textContent = "Active";
    heading.append(" ", activePill);

    const models = store.models().filter((entry) => !entry.draft);
    const counts = document.createElement("p");
    counts.className = "profile-counts";
    const local = models.filter((entry) => entry.kind === "local").length;
    const remote = models.filter((entry) => entry.kind === "remote").length;
    counts.textContent = `${local} local model${local === 1 ? "" : "s"} \u00b7 ${remote} remote model${remote === 1 ? "" : "s"}`;
    pane.append(counts);

    pane.append(allowlistSummary(), chainEditor());
    return pane;
  };

  const allowlistSummary = (): HTMLElement => {
    const box = document.createElement("div");
    box.className = "profile-allowlist";
    const heading = document.createElement("h3");
    heading.className = "profiles-heading";
    heading.textContent = "Allowlist";
    box.append(heading);
    const list = store.allowlist();
    if (list === null) {
      const all = document.createElement("p");
      all.className = "field-help";
      all.textContent = "All models visible.";
      box.append(all);
      return box;
    }
    const chips = document.createElement("div");
    chips.className = "pill-row";
    for (const nameText of list) {
      const chip = document.createElement("span");
      chip.className = "pill";
      chip.textContent = nameText;
      chips.append(chip);
    }
    box.append(chips);
    return box;
  };

  const chainEditor = (): HTMLElement => {
    const box = document.createElement("section");
    box.className = "include-chain";
    box.setAttribute("aria-label", "Include chain");
    const heading = document.createElement("h3");
    heading.className = "profiles-heading";
    heading.textContent = "Include chain";
    box.append(heading);

    if (chainError !== null) {
      box.append(errorBanner(chainError));
    }

    const rows = chainRows();
    if (rows.length === 0) {
      const empty = document.createElement("p");
      empty.className = "view-empty";
      empty.textContent = "No includes: this profile stands alone.";
      box.append(empty);
    } else {
      const list = document.createElement("ol");
      list.className = "chain-list";
      rows.forEach((row, index) => list.append(chainRow(row, index, rows.length)));
      box.append(list);
    }

    // Add Include: autocomplete over the existing .toml files not yet
    // in the chain, plus free text that creates a new empty file.
    const addRow = document.createElement("div");
    addRow.className = "chain-add";
    const addLabel = document.createElement("label");
    addLabel.setAttribute("for", "chain-add-input");
    addLabel.className = "visually-hidden";
    addLabel.textContent = "Include file to add";
    const input = document.createElement("input");
    input.id = "chain-add-input";
    input.className = "input chain-add-input";
    input.placeholder = "common.toml or a new file name";
    const listId = "chain-add-options";
    input.setAttribute("list", listId);
    const options = document.createElement("datalist");
    options.id = listId;
    const inChain = new Set(rows.map((row) => row.path));
    for (const profileName of profiles ?? []) {
      const file = `${profileName}.toml`;
      if (!inChain.has(file) && profileName !== store.activeProfile) {
        const option = document.createElement("option");
        option.value = file;
        options.append(option);
      }
    }
    const add = document.createElement("button");
    add.type = "button";
    add.className = "button button-xs button-outline chain-add-button";
    add.textContent = "Add Include";
    add.addEventListener("click", () => {
      if (input.value.trim().length > 0) {
        void addInclude(input.value);
      }
    });
    addRow.append(addLabel, input, options, add);
    box.append(addRow);

    const orderNote = document.createElement("p");
    orderNote.className = "field-help chain-note";
    orderNote.textContent = "Later files override earlier ones.";
    box.append(orderNote);

    const save = document.createElement("button");
    save.type = "button";
    save.className = "button button-primary chain-save";
    save.textContent = "Save Chain";
    save.disabled = chainEdit === null;
    save.addEventListener("click", () => void saveChain());
    box.append(save);
    return box;
  };

  const chainRow = (row: ChainFile, index: number, count: number): HTMLElement => {
    const item = document.createElement("li");
    item.className = "chain-row";
    const missing = isMissing(row);
    if (missing) {
      item.classList.add("is-missing");
    }

    const path = document.createElement("span");
    path.className = "chain-path";
    path.textContent = row.path;
    item.append(path);

    if (missing) {
      // The accepted-risk indicator: a deleted parent stays visible.
      const pill = document.createElement("span");
      pill.className = "pill chain-missing";
      pill.textContent = "Missing";
      item.append(pill);
    }

    const actions = document.createElement("span");
    actions.className = "chain-actions";
    // Reorder controls: up/down buttons rather than HTML5 drag - real
    // buttons are keyboard-operable and labeled, which draggable rows
    // are not without a parallel keyboard path.
    const up = iconButton(ArrowUp, `Move ${row.path} up`, () => moveRow(index, -1));
    up.classList.add("chain-up");
    up.disabled = index === 0;
    const down = iconButton(ArrowDown, `Move ${row.path} down`, () => moveRow(index, 1));
    down.classList.add("chain-down");
    down.disabled = index === count - 1;
    actions.append(up, down);

    if (!row.outside && !missing) {
      const edit = document.createElement("a");
      edit.className = "button button-xs button-outline chain-edit";
      edit.href = drillHref(row.path);
      edit.textContent = "Edit";
      edit.setAttribute("aria-label", `Edit ${row.path}`);
      actions.append(edit);
    }

    // An outside row (the boot file) offers no remove: the merged
    // chain needs the boot-owned sections it carries, so a save
    // without it can never validate, and the add input refuses `../`
    // paths, so the removal could not be undone short of a reload.
    if (!row.outside) {
      const remove = iconButton(X, `Remove ${row.path} from the chain`, () => removeRow(index));
      remove.classList.add("chain-remove");
      actions.append(remove);
    }
    item.append(actions);
    return item;
  };

  const renderDrillIn = (target: string): HTMLElement => {
    const root = document.createElement("div");
    root.className = "profiles-view include-drill";

    if (!store.loaded) {
      const skeleton = document.createElement("div");
      skeleton.className = "skeleton-row";
      skeleton.setAttribute("aria-hidden", "true");
      root.append(skeleton);
      main?.replaceChildren(root);
      return root;
    }

    const row =
      store.includeChain().find((file) => file.path === target || file.base === target) ?? null;

    const crumbs = document.createElement("nav");
    crumbs.className = "breadcrumbs";
    crumbs.setAttribute("aria-label", "Breadcrumb");
    const backCrumb = document.createElement("a");
    backCrumb.className = "crumb";
    backCrumb.href = "#/profiles";
    backCrumb.textContent = `${store.activeProfile}.toml`;
    const separator = document.createElement("span");
    separator.className = "crumb-separator";
    separator.setAttribute("aria-hidden", "true");
    separator.textContent = "\u203a";
    const current = document.createElement("span");
    current.className = "crumb crumb-current";
    current.setAttribute("aria-current", "page");
    current.textContent = row?.base ?? target;
    crumbs.append(backCrumb, separator, current);
    root.append(crumbs);

    const title = document.createElement("h1");
    title.className = "view-title";
    title.textContent = row?.base ?? target;
    root.append(title);

    if (row === null) {
      root.append(
        errorBanner(
          `${target} is not part of the active profile's include chain, so it cannot be edited here.`,
        ),
      );
      main?.replaceChildren(root);
      return root;
    }

    if (drillError !== null) {
      root.append(errorBanner(drillError));
    }

    const body = store.includeFileBody(row.base);
    const keys = Object.keys(body).sort();
    if (drillEdits.size === 0) {
      for (const key of keys) {
        drillEdits.set(key, JSON.stringify(body[key], null, 2));
      }
    }

    const note = document.createElement("p");
    note.className = "field-help drill-note";
    // The generic editor's honest contract, stated where it is used.
    note.textContent =
      "This file's content, derived from the pending view: values another include later overrides are not shown and are dropped from the saved shadow. Secrets stay *** and are preserved on save.";
    root.append(note);

    if (drillEdits.size === 0) {
      const empty = document.createElement("p");
      empty.className = "view-empty";
      empty.textContent = "No pending values are attributed to this file.";
      root.append(empty);
      main?.replaceChildren(root);
      return root;
    }

    const form = document.createElement("div");
    form.className = "drill-form";
    for (const [key, text] of drillEdits) {
      const field = document.createElement("div");
      field.className = "field-row drill-field";
      field.dataset["key"] = key;
      const label = document.createElement("label");
      label.setAttribute("for", `include-${key}`);
      label.textContent = key;
      const area = document.createElement("textarea");
      area.id = `include-${key}`;
      area.className = "input drill-editor";
      area.rows = Math.min(16, Math.max(3, text.split("\n").length));
      area.value = text;
      area.addEventListener("input", () => drillEdits.set(key, area.value));
      field.append(label, area);
      form.append(field);
    }
    root.append(form);

    const save = document.createElement("button");
    save.type = "button";
    save.className = "button button-primary drill-save";
    save.textContent = "Save";
    save.addEventListener("click", () => void saveDrill(row));
    root.append(save);

    main?.replaceChildren(root);
    return root;
  };

  return {
    mount(target: HTMLElement, includePath?: string): void {
      main = target;
      const nextDrill = includePath ?? null;
      if (nextDrill !== drillPath) {
        drillPath = nextDrill;
        drillEdits.clear();
        drillError = null;
      }
      if (profiles === null && listError === null) {
        void refreshProfiles();
      }
      render();
    },
  };

  /** A small icon-only action button with an accessible name. */
  function iconButton(
    icon: Parameters<typeof lucideElement>[0],
    label: string,
    action: () => void,
  ): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "field-reset chain-action";
    button.setAttribute("aria-label", label);
    button.append(lucideElement(icon, { "aria-hidden": "true", width: 14, height: 14 }));
    button.addEventListener("click", action);
    return button;
  }

  /** The view-top failure banner, with an optional Retry. */
  function errorBanner(message: string, retry?: () => void): HTMLElement {
    const banner = document.createElement("div");
    banner.className = "banner banner-danger";
    banner.setAttribute("role", "alert");
    const text = document.createElement("span");
    text.textContent = message;
    banner.append(text);
    if (retry) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "button button-xs button-outline";
      button.textContent = "Retry";
      button.addEventListener("click", retry);
      banner.append(button);
    }
    return banner;
  }
}

/**
 * The New Profile dialog [Adapted: Unsloth] dialog + radio-group: name
 * input, a Start-from radio group (Empty / Copy of / Include), inline
 * validation mirroring the server's name rule, and server refusals
 * surfaced in place so the dialog survives a 409.
 */
function newProfileDialog(
  host: HTMLElement,
  api: GatewayApi,
  profiles: string[],
  onCreated: (name: string) => void,
): void {
  const overlay = document.createElement("div");
  overlay.className = "overlay dialog-overlay";
  const card = document.createElement("section");
  card.className = "modal new-profile-dialog";
  card.setAttribute("role", "dialog");
  card.setAttribute("aria-modal", "true");

  const heading = document.createElement("h2");
  heading.id = "new-profile-title";
  heading.textContent = "New Profile";
  card.setAttribute("aria-labelledby", heading.id);

  const form = document.createElement("form");

  const nameField = document.createElement("div");
  nameField.className = "field-row";
  const nameLabel = document.createElement("label");
  nameLabel.setAttribute("for", "new-profile-name");
  nameLabel.textContent = "Name";
  const nameInput = document.createElement("input");
  nameInput.id = "new-profile-name";
  nameInput.className = "input";
  nameInput.autocomplete = "off";
  nameField.append(nameLabel, nameInput);

  const error = document.createElement("p");
  error.className = "field-error dialog-error";
  error.setAttribute("aria-live", "polite");
  error.hidden = true;

  const modes = document.createElement("fieldset");
  modes.className = "start-from";
  const legend = document.createElement("legend");
  legend.textContent = "Start from";
  modes.append(legend);

  let copyValue = profiles[0] ?? "";
  const copyFrom = createDropdownControl({
    id: "new-profile-copy-from",
    options: profiles.map((profile) => ({ value: profile, label: profile })),
    value: copyValue,
    onChange: (value) => {
      copyValue = value;
    },
  });
  copyFrom.trigger.classList.add("select-sm");
  copyFrom.trigger.setAttribute("aria-label", "Profile to copy");
  copyFrom.setDisabled(true);
  let includeValue = profiles[0] ?? "";
  const includeFrom = createDropdownControl({
    id: "new-profile-include-from",
    options: profiles.map((profile) => ({ value: profile, label: profile })),
    value: includeValue,
    onChange: (value) => {
      includeValue = value;
    },
  });
  includeFrom.trigger.classList.add("select-sm");
  includeFrom.trigger.setAttribute("aria-label", "Profile to include");
  includeFrom.setDisabled(true);

  const radioRow = (
    value: string,
    labelText: string,
    checked: boolean,
    extra?: HTMLElement,
  ): HTMLElement => {
    const rowBox = document.createElement("div");
    rowBox.className = "radio-row";
    const radio = document.createElement("input");
    radio.type = "radio";
    radio.name = "start-from";
    radio.value = value;
    radio.id = `start-from-${value}`;
    radio.checked = checked;
    const radioLabel = document.createElement("label");
    radioLabel.setAttribute("for", radio.id);
    radioLabel.textContent = labelText;
    rowBox.append(radio, radioLabel);
    if (extra) {
      rowBox.append(extra);
    }
    return rowBox;
  };
  modes.append(
    radioRow("empty", "Empty", true),
    radioRow("copy", "Copy of", false, copyFrom.element),
    radioRow("include", "Include", false, includeFrom.element),
  );
  // The mode's dropdown enables with its radio, so a disabled select
  // never carries the submitted choice.
  modes.addEventListener("change", () => {
    const mode = modes.querySelector<HTMLInputElement>("input[name='start-from']:checked")?.value;
    copyFrom.setDisabled(mode !== "copy");
    includeFrom.setDisabled(mode !== "include");
  });

  const actions = document.createElement("div");
  actions.className = "modal-actions";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "button button-outline";
  cancel.textContent = "Cancel";
  const submit = document.createElement("button");
  submit.type = "submit";
  submit.className = "button button-primary";
  submit.textContent = "Create";
  actions.append(cancel, submit);

  form.append(nameField, error, modes, actions);
  card.append(heading, form);
  overlay.append(card);
  host.append(overlay);

  // Duck-typed: the HTMLElement global is absent under node --test.
  const opener = document.activeElement as HTMLElement | null;
  const restore = opener && typeof opener.focus === "function" ? opener : null;
  const close = (): void => {
    overlay.remove();
    if (restore?.isConnected) {
      restore.focus();
    }
  };

  const showError = (message: string): void => {
    error.textContent = message;
    error.hidden = false;
  };

  cancel.addEventListener("click", close);
  overlay.addEventListener("click", (event) => {
    if (event.target === overlay) {
      close();
    }
  });
  card.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      close();
      return;
    }
    // The focus trap the confirm modal establishes, generalized to the
    // form's control count: Tab at either edge wraps instead of
    // escaping the aria-modal dialog into the inert chrome.
    if (event.key === "Tab") {
      const controls = [...card.querySelectorAll<HTMLElement>("input, button")].filter(
        (entry) =>
          !(entry as HTMLInputElement).disabled &&
          entry.closest<HTMLElement>("[hidden]") === null,
      );
      const first = controls[0];
      const last = controls[controls.length - 1];
      if (!first || !last) {
        return;
      }
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
  });

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const name = nameInput.value.trim();
    // Client-side mirror of the server's trust-boundary rule; a name
    // the server would refuse never leaves the dialog.
    const nameError = profileNameError(name);
    if (nameError !== null) {
      showError(nameError);
      return;
    }
    const mode =
      modes.querySelector<HTMLInputElement>("input[name='start-from']:checked")?.value ?? "empty";
    let body: CreateProfileBody;
    if (mode === "copy") {
      body = { mode: "copy", from: copyValue };
    } else if (mode === "include") {
      body = { mode: "include", from: includeValue };
    } else {
      body = { mode: "empty" };
    }
    void (async () => {
      try {
        await api.createProfile(name, body);
      } catch (requestError) {
        if (requestError instanceof UnauthorizedError) {
          return;
        }
        // A 409 (exists) or 400/404 refusal keeps the dialog open with
        // the server's sentence in place.
        showError(
          requestError instanceof GatewayHttpError
            ? requestError.message
            : "The profile could not be created",
        );
        return;
      }
      close();
      onCreated(name);
    })();
  });

  nameInput.focus();
}
