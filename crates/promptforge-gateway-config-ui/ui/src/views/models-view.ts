// The Models view [Unsloth] Model Hub "On Device": a master-detail
// split. The list side carries the toolbar (debounced search, filter
// chips, allowlist chip, sort), the model rows with status dots and
// badges, the unconfigured-orphans section, the empty state, and
// loading skeletons. The detail side renders the selected model's
// header (editable name, status, badges, source with reveal, the
// Save/Reset/Delete action row) and the registry-driven sections with
// per-field dirty dots, reset buttons, and pending chips.

import {
  Cloud,
  Cpu,
  FolderOpen,
  RotateCcw,
  X,
  createElement as lucideElement,
} from "lucide";

import { confirmDialog } from "../components/confirm-modal";
import { createChipInput } from "../components/chip-input";
import { createDropdownControl } from "../components/dropdown-control";
import { createSliderControl } from "../components/slider-control";
import type { SliderControl } from "../components/slider-control";
import { createToggleControl } from "../components/toggle-control";
import {
  LOCAL_MODEL_SECTIONS,
  LOCAL_MODEL_SETTINGS,
  REMOTE_MODEL_SECTIONS,
  REMOTE_MODEL_SETTINGS,
  settingOptions,
} from "../components/settings-registry";
import type { SectionDef, SettingContext, SettingDef } from "../components/settings-registry";
import type { ToastStack } from "../components/toast";
import type { ConfigStore, ModelEntry } from "../services/config-store";
import type { GatewayApi, OrphanFile } from "../services/gateway-api";

/** How long the search input waits before filtering. */
const SEARCH_DEBOUNCE_MS = 150;

/** The list filters, in chip order. */
const FILTERS = ["all", "local", "remote", "unconfigured"] as const;
type Filter = (typeof FILTERS)[number];

/** The sort orders the toolbar offers. */
type Sort = "name" | "size" | "kind";

/** Construction dependencies for the view. */
export interface ModelsViewDeps {
  /** The config store: catalog, edits, save path. */
  store: ConfigStore;
  /** The admin API, for model-info, reveal, and cache deletes. */
  api: GatewayApi;
  /** Outcome surfacing. */
  toasts: ToastStack;
}

/** The mounted view handle the router calls. */
export interface ModelsView {
  /** Renders the view into `main`, selecting `selected` when given. */
  mount(main: HTMLElement, selected?: string): void;
}

/** Builds the Models view (state survives route re-mounts). */
export function createModelsView(deps: ModelsViewDeps): ModelsView {
  const { store, api, toasts } = deps;

  let search = "";
  let filter: Filter = "all";
  let allowlistOnly = false;
  let sort: Sort = "name";
  /** Collapsed detail sections, keyed `name:section`. */
  const collapsed = new Set<string>();
  /** GGUF layer totals by model name; null = lookup failed (plain N). */
  const layerTotals = new Map<string, number | null>();
  /** The entry whose inherited-edit note is currently visible. */
  let inheritNoteFor: string | null = null;

  let main: HTMLElement | null = null;
  /** The last-rendered split root; a re-render is legal only while it owns `main`. */
  let viewRoot: HTMLElement | null = null;
  let selected: string | undefined;
  let listBox: HTMLElement | null = null;
  let detailBox: HTMLElement | null = null;
  let searchTimer: ReturnType<typeof setTimeout> | null = null;

  store.subscribe(() => {
    // Guard on this view's own root, not just `main`: `main` is shared
    // with every other view, so a store notification arriving while
    // another view owns it must not let this one repaint the pane.
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

  const render = (): void => {
    if (!main) {
      return;
    }
    const title = document.createElement("h1");
    title.className = "view-title";
    title.textContent = "Models";

    const split = document.createElement("div");
    split.className = "split models-split";
    listBox = document.createElement("div");
    listBox.className = "split-list";
    detailBox = document.createElement("div");
    detailBox.className = "split-detail";
    split.append(listBox, detailBox);

    renderList();
    renderDetail();
    viewRoot = split;
    main.replaceChildren(title, split);
  };

  // ----- the list side ---------------------------------------------------

  const renderList = (): void => {
    if (!listBox) {
      return;
    }
    if (!store.loaded) {
      listBox.replaceChildren(skeletonList());
      return;
    }
    if (store.loadError) {
      listBox.replaceChildren(loadErrorBanner(store.loadError));
      return;
    }
    const entries = visibleEntries();
    const parts: HTMLElement[] = [buildToolbar()];
    const all = store.models();
    if (all.length === 0 && store.orphans.length === 0) {
      parts.push(emptyState());
    } else if (filter !== "unconfigured") {
      parts.push(modelList(entries));
    }
    if ((filter === "all" || filter === "unconfigured") && store.orphans.length > 0) {
      parts.push(orphanSection(store.orphans));
    }
    listBox.replaceChildren(...parts);
  };

  const visibleEntries = (): ModelEntry[] => {
    let entries = store.models();
    if (filter === "local" || filter === "remote") {
      entries = entries.filter((entry) => entry.kind === filter);
    }
    const query = search.trim().toLowerCase();
    if (query !== "") {
      entries = entries.filter((entry) => entry.name.toLowerCase().includes(query));
    }
    const allowlist = store.allowlist();
    if (allowlistOnly && allowlist) {
      entries = entries.filter((entry) => allowlist.includes(entry.name));
    }
    const byName = (a: ModelEntry, b: ModelEntry): number => a.name.localeCompare(b.name);
    if (sort === "kind") {
      entries = [...entries].sort((a, b) => a.kind.localeCompare(b.kind) || byName(a, b));
    } else {
      // Configured entries carry no file size; the size sort falls back
      // to name order until sizes are known.
      entries = [...entries].sort(byName);
    }
    return entries;
  };

  const buildToolbar = (): HTMLElement => {
    const toolbar = document.createElement("div");
    toolbar.className = "models-toolbar";

    const searchLabel = document.createElement("label");
    searchLabel.className = "visually-hidden";
    searchLabel.htmlFor = "models-search";
    searchLabel.textContent = "Search models";
    const searchInput = document.createElement("input");
    searchInput.type = "search";
    searchInput.id = "models-search";
    searchInput.className = "input";
    searchInput.placeholder = "Search models";
    searchInput.value = search;
    searchInput.addEventListener("input", () => {
      if (searchTimer !== null) {
        clearTimeout(searchTimer);
      }
      searchTimer = setTimeout(() => {
        search = searchInput.value;
        renderList();
        // The toolbar re-renders with the list; put focus back where
        // the user is typing.
        listBox?.querySelector<HTMLInputElement>("#models-search")?.focus();
      }, SEARCH_DEBOUNCE_MS);
      (searchTimer as unknown as { unref?: () => void }).unref?.();
    });

    const chips = document.createElement("div");
    chips.className = "filter-chips";
    chips.setAttribute("role", "group");
    chips.setAttribute("aria-label", "Filter models");
    for (const value of FILTERS) {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.className = "pill filter-chip";
      chip.dataset["filter"] = value;
      chip.setAttribute("aria-pressed", String(filter === value));
      chip.textContent = value === "all" ? "All" : value[0]?.toUpperCase() + value.slice(1);
      chip.addEventListener("click", () => {
        filter = value;
        renderList();
      });
      chips.append(chip);
    }

    const allowlist = store.allowlist();
    const allowChip = document.createElement("button");
    allowChip.type = "button";
    allowChip.className = "pill filter-chip allowlist-chip";
    if (allowlist === null) {
      allowChip.disabled = true;
      allowChip.textContent = "All models visible";
      allowChip.title = "This profile has no model allowlist.";
    } else {
      allowChip.textContent = `Allowlist (${allowlist.length})`;
      allowChip.setAttribute("aria-pressed", String(allowlistOnly));
      allowChip.title = "Restrict the list to the profile's allowlisted models.";
      allowChip.addEventListener("click", () => {
        allowlistOnly = !allowlistOnly;
        renderList();
      });
    }

    const sortLabel = document.createElement("label");
    sortLabel.className = "visually-hidden";
    sortLabel.htmlFor = "models-sort";
    sortLabel.textContent = "Sort models";
    const sortSelect = document.createElement("select");
    sortSelect.id = "models-sort";
    sortSelect.className = "select select-sm";
    for (const [value, label] of [
      ["name", "Name"],
      ["size", "Size"],
      ["kind", "Kind"],
    ] as const) {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      sortSelect.append(option);
    }
    sortSelect.value = sort;
    sortSelect.addEventListener("change", () => {
      sort = sortSelect.value as Sort;
      renderList();
    });

    const addLocal = document.createElement("button");
    addLocal.type = "button";
    addLocal.className = "button button-xs button-outline toolbar-add-local";
    addLocal.textContent = "Add Local";
    addLocal.addEventListener("click", () => addModel("local"));
    const addRemote = document.createElement("button");
    addRemote.type = "button";
    addRemote.className = "button button-xs button-outline toolbar-add-remote";
    addRemote.textContent = "Add Remote";
    addRemote.addEventListener("click", () => addModel("remote"));

    toolbar.append(searchLabel, searchInput, chips, allowChip, sortLabel, sortSelect, addLocal, addRemote);
    return toolbar;
  };

  const modelList = (entries: ModelEntry[]): HTMLElement => {
    const list = document.createElement("ul");
    list.className = "model-list";
    for (const entry of entries) {
      const row = document.createElement("li");
      const link = document.createElement("a");
      link.className = "model-row";
      link.href = `#/models/${encodeURIComponent(entry.name)}`;
      if (entry.name === selected) {
        link.setAttribute("aria-current", "true");
      }

      link.append(statusDot(entry.name));

      const name = document.createElement("span");
      name.className = "model-name";
      name.textContent = entry.name;
      link.append(name);

      const kindBadge = document.createElement("span");
      kindBadge.className = "pill kind-badge";
      kindBadge.textContent = String(entry.data["kind"] ?? "chat");
      link.append(kindBadge);

      const quant = quantFromSource(entry);
      if (quant) {
        const quantBadge = document.createElement("span");
        quantBadge.className = "pill pill-accent quant-badge";
        quantBadge.textContent = quant;
        link.append(quantBadge);
      }

      if (entry.draft) {
        const draftBadge = document.createElement("span");
        draftBadge.className = "pill draft-badge";
        draftBadge.textContent = "unsaved";
        link.append(draftBadge);
      }

      link.append(sourceIcon(entry.kind));
      row.append(link);
      list.append(row);
    }
    return list;
  };

  const orphanSection = (orphans: OrphanFile[]): HTMLElement => {
    const section = document.createElement("section");
    section.className = "orphan-section";
    const heading = document.createElement("h2");
    heading.className = "orphan-heading";
    heading.textContent = "Unconfigured files on disk";
    const list = document.createElement("ul");
    list.className = "orphan-list";
    for (const orphan of orphans) {
      const row = document.createElement("li");
      row.className = "orphan-row";

      const name = document.createElement("span");
      name.className = "model-name";
      name.textContent = fileName(orphan.path);

      const size = document.createElement("span");
      size.className = "orphan-size";
      size.textContent = formatBytes(orphan.size_bytes);

      const adopt = document.createElement("button");
      adopt.type = "button";
      adopt.className = "button button-xs button-outline orphan-adopt";
      adopt.textContent = "Adopt";
      adopt.addEventListener("click", () => adoptOrphan(orphan));

      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "button button-xs button-danger orphan-delete";
      remove.textContent = "Delete";
      if (orphan.sha256 === null) {
        remove.disabled = true;
        remove.title = "This file was not downloaded by the cache; delete it on disk.";
      } else {
        remove.addEventListener("click", () => void deleteOrphan(orphan));
      }

      row.append(name, size, adopt, remove);
      list.append(row);
    }
    section.append(heading, list);
    return section;
  };

  const adoptOrphan = (orphan: OrphanFile): void => {
    const data: Record<string, unknown> = {
      name: fileName(orphan.path).replace(/\.gguf$/i, ""),
      kind: "chat",
      description: "",
      source: orphan.path,
      context: 4096,
    };
    if (orphan.sha256 !== null) {
      data["sha256"] = orphan.sha256;
    }
    const name = store.addDraft("local", data);
    navigate(`#/models/${encodeURIComponent(name)}`);
  };

  const deleteOrphan = async (orphan: OrphanFile): Promise<void> => {
    const yes = await confirmDialog(document.body, {
      title: "Delete file?",
      body: `Delete ${fileName(orphan.path)} (${formatBytes(orphan.size_bytes)}) from the cache. This cannot be undone.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!yes || orphan.sha256 === null) {
      return;
    }
    try {
      await api.deleteCached(orphan.sha256);
    } catch (error) {
      toasts.show(error instanceof Error ? error.message : "The delete failed", "error");
      return;
    }
    toasts.show(`Deleted ${fileName(orphan.path)}`, "success");
    await store.refreshOrphans();
  };

  const emptyState = (): HTMLElement => {
    const empty = document.createElement("div");
    empty.className = "view-empty empty-state";
    const message = document.createElement("p");
    message.textContent = "No models configured";

    const actions = document.createElement("div");
    actions.className = "empty-actions";
    const addLocal = document.createElement("button");
    addLocal.type = "button";
    addLocal.className = "button button-primary";
    addLocal.textContent = "Add Local Model";
    addLocal.addEventListener("click", () => addModel("local"));
    const addRemote = document.createElement("button");
    addRemote.type = "button";
    addRemote.className = "button button-outline";
    addRemote.textContent = "Add Remote Model";
    addRemote.addEventListener("click", () => addModel("remote"));
    const discover = document.createElement("a");
    discover.className = "button button-outline";
    discover.href = "#/discover";
    discover.textContent = "Search Hugging Face";

    actions.append(addLocal, addRemote, discover);
    empty.append(message, actions);
    return empty;
  };

  const addModel = (kind: "local" | "remote"): void => {
    const data: Record<string, unknown> =
      kind === "local"
        ? { name: "new-local-model", kind: "chat", description: "", source: "", context: 4096 }
        : {
            name: "new-remote-model",
            kind: "chat",
            description: "",
            context: 4096,
            upstream: "",
            endpoints: [],
          };
    const name = store.addDraft(kind, data);
    navigate(`#/models/${encodeURIComponent(name)}`);
  };

  const skeletonList = (): HTMLElement => {
    const list = document.createElement("ul");
    list.className = "model-list";
    list.setAttribute("aria-hidden", "true");
    for (let i = 0; i < 4; i += 1) {
      const row = document.createElement("li");
      row.className = "skeleton-row";
      list.append(row);
    }
    return list;
  };

  const loadErrorBanner = (message: string): HTMLElement => {
    const banner = document.createElement("div");
    banner.className = "banner banner-danger";
    const text = document.createElement("span");
    text.textContent = `Could not load the configuration: ${message}`;
    const retry = document.createElement("button");
    retry.type = "button";
    retry.className = "button button-xs button-outline";
    retry.textContent = "Retry";
    retry.addEventListener("click", () => void store.load());
    banner.append(text, retry);
    return banner;
  };

  // ----- the detail side --------------------------------------------------

  const renderDetail = (): void => {
    if (!detailBox) {
      return;
    }
    if (!store.loaded || store.loadError) {
      detailBox.replaceChildren();
      return;
    }
    if (!selected) {
      const hint = document.createElement("p");
      hint.className = "view-empty";
      hint.textContent = "Select a model to edit its settings.";
      detailBox.replaceChildren(hint);
      return;
    }
    const entry = store.findByName(selected);
    if (!entry) {
      const missing = document.createElement("p");
      missing.className = "view-empty";
      missing.textContent = `No model named ${selected}.`;
      detailBox.replaceChildren(missing);
      return;
    }
    const parts: HTMLElement[] = [];
    if (inheritNoteFor === entry.name) {
      parts.push(inheritNote(entry));
    }
    parts.push(detailHeader(entry));
    const sections = entry.kind === "local" ? LOCAL_MODEL_SECTIONS : REMOTE_MODEL_SECTIONS;
    const settings = entry.kind === "local" ? LOCAL_MODEL_SETTINGS : REMOTE_MODEL_SETTINGS;
    for (const section of sections) {
      parts.push(renderSection(entry, section, settings));
    }
    detailBox.replaceChildren(...parts);
  };

  const contextFor = (entry: ModelEntry): SettingContext => ({
    value: (key) => store.value(entry, key),
    dominions: () => store.dominions(),
    endpointIds: () => store.endpointIds(),
  });

  const commit = (entry: ModelEntry, key: string, value: unknown): void => {
    if (store.takeInheritNote(entry)) {
      inheritNoteFor = entry.name;
    }
    store.setEdit(entry, key, value);
  };

  // Inherited-edit override note [INVENTED]: part of the plan's
  // inheritance UX; no researched UI has include-chain inheritance.
  const inheritNote = (entry: ModelEntry): HTMLElement => {
    const note = document.createElement("div");
    note.className = "banner inherit-note";
    const text = document.createElement("span");
    const leaf = store.activeProfile ? `${store.activeProfile}.toml` : "the active profile";
    text.textContent =
      `This copies the full model definition into ${leaf} as an override. ` +
      `To change it for every profile that includes ${entry.sourceFile ?? "the parent file"}, ` +
      `edit ${entry.sourceFile ?? "the parent file"} instead.`;
    const dismiss = document.createElement("button");
    dismiss.type = "button";
    dismiss.className = "button button-xs button-outline";
    dismiss.setAttribute("aria-label", "Dismiss");
    dismiss.append(lucideElement(X, { "aria-hidden": "true", width: 12, height: 12 }));
    dismiss.addEventListener("click", () => {
      inheritNoteFor = null;
      note.remove();
    });
    note.append(text, dismiss);
    return note;
  };

  const detailHeader = (entry: ModelEntry): HTMLElement => {
    const header = document.createElement("header");
    header.className = "detail-header";

    const nameLabel = document.createElement("label");
    nameLabel.className = "visually-hidden";
    nameLabel.htmlFor = "detail-name";
    nameLabel.textContent = "Model name";
    const nameInput = document.createElement("input");
    nameInput.type = "text";
    nameInput.id = "detail-name";
    nameInput.className = "detail-title";
    nameInput.value = String(store.value(entry, "name") ?? entry.name);
    nameInput.addEventListener("change", () => commit(entry, "name", nameInput.value));

    const meta = document.createElement("div");
    meta.className = "detail-meta";
    meta.append(statusDot(entry.name));
    const statusText = document.createElement("span");
    statusText.className = "detail-status";
    statusText.textContent = entry.draft
      ? "Unsaved"
      : store.isRunning(entry.name)
        ? "Running"
        : "Stopped";
    meta.append(statusText);

    const kindBadge = document.createElement("span");
    kindBadge.className = "pill kind-badge";
    kindBadge.textContent = String(store.value(entry, "kind") ?? "chat");
    meta.append(kindBadge);

    const quant = quantFromSource(entry);
    if (quant) {
      const quantBadge = document.createElement("span");
      quantBadge.className = "pill pill-accent quant-badge";
      quantBadge.textContent = quant;
      meta.append(quantBadge);
    }
    meta.append(sourceIcon(entry.kind));

    header.append(nameLabel, nameInput, meta);

    // Provenance annotation [INVENTED]: no researched UI has
    // include-chain inheritance; flagged per the plan.
    if (entry.inherited && entry.sourceFile) {
      const from = document.createElement("p");
      from.className = "field-from";
      from.textContent = `from ${entry.sourceFile}`;
      // The include-file drill-in lands with the include editor; until
      // then the breadcrumb names the chain.
      from.title = `${store.activeProfile}.toml > ${entry.sourceFile}`;
      header.append(from);
    }

    const source = typeof entry.data["source"] === "string" ? entry.data["source"] : null;
    if (entry.kind === "local" && source) {
      const sourceRow = document.createElement("p");
      sourceRow.className = "detail-source";
      const path = document.createElement("span");
      path.className = "model-name";
      path.textContent = source;
      sourceRow.append(path);
      if (!/^[a-z]+:\/\//i.test(source)) {
        const reveal = document.createElement("button");
        reveal.type = "button";
        reveal.className = "button button-xs button-outline reveal-button";
        reveal.setAttribute("aria-label", "Reveal in file manager");
        reveal.append(lucideElement(FolderOpen, { "aria-hidden": "true", width: 14, height: 14 }));
        reveal.addEventListener("click", () => {
          void api.reveal(source).catch((error: unknown) => {
            toasts.show(error instanceof Error ? error.message : "The reveal failed", "error");
          });
        });
        sourceRow.append(reveal);
      }
      header.append(sourceRow);
    }

    header.append(
      fieldRow(entry, {
        key: "kind",
        label: "Kind",
        help: "The workload this model serves.",
        section: "header",
        type: "dropdown",
        options: ["chat", "embedding", "classifier"],
        default: "chat",
      }),
      fieldRow(entry, {
        key: "description",
        label: "Description",
        help: "Prose describing the model for catalog consumers.",
        section: "header",
        type: "textarea",
        default: "",
      }),
    );

    const actions = document.createElement("div");
    actions.className = "detail-actions";

    const save = document.createElement("button");
    save.type = "button";
    save.className = "button button-primary detail-save";
    save.textContent = "Save";
    save.disabled = !store.hasEdits(entry);
    save.addEventListener("click", () => {
      save.disabled = true;
      void (async () => {
        const savedName = String(store.value(entry, "name") ?? entry.name);
        try {
          await store.save(entry);
        } catch (error) {
          toasts.show(error instanceof Error ? error.message : "The save failed", "error");
          render();
          return;
        }
        toasts.show("Saved to disk", "success");
        if (savedName !== selected) {
          navigate(`#/models/${encodeURIComponent(savedName)}`);
        }
      })();
    });

    const reset = document.createElement("button");
    reset.type = "button";
    reset.className = "button button-outline detail-reset";
    reset.textContent = "Reset";
    reset.disabled = entry.draft || !store.hasEdits(entry);
    reset.addEventListener("click", () => store.resetEntry(entry));

    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "button button-danger detail-delete";
    remove.textContent = entry.draft ? "Discard" : "Delete";
    remove.addEventListener("click", () => {
      void (async () => {
        if (entry.draft) {
          store.discardDraft(entry);
          navigate("#/models");
          return;
        }
        const yes = await confirmDialog(document.body, {
          title: `Delete ${entry.name}?`,
          body: `Remove the model ${entry.name} from the configuration. The change is staged until you apply it.`,
          confirmLabel: "Delete",
          danger: true,
        });
        if (!yes) {
          return;
        }
        try {
          await store.deleteModel(entry);
        } catch (error) {
          toasts.show(error instanceof Error ? error.message : "The delete failed", "error");
          return;
        }
        toasts.show(`Deleted ${entry.name}`, "success");
        navigate("#/models");
      })();
    });

    actions.append(save, reset, remove);
    header.append(actions);
    return header;
  };

  const renderSection = (
    entry: ModelEntry,
    section: SectionDef,
    settings: readonly SettingDef[],
  ): HTMLElement => {
    const wrap = document.createElement("section");
    wrap.className = "detail-section";
    wrap.dataset["section"] = section.id;

    if (section.presentKey && store.value(entry, section.presentKey) == null) {
      const add = document.createElement("button");
      add.type = "button";
      add.className = "button button-outline section-add";
      add.textContent = section.addLabel ?? `Add ${section.label}`;
      add.addEventListener("click", () => {
        commit(entry, section.presentKey ?? "", section.addValue?.() ?? {});
      });
      wrap.append(add);
      return wrap;
    }

    const collapseKey = `${entry.name}:${section.id}`;
    const heading = document.createElement("h3");
    heading.className = "section-heading";
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "section-toggle";
    toggle.setAttribute("aria-expanded", String(!collapsed.has(collapseKey)));
    toggle.textContent = section.label;
    heading.append(toggle);

    const body = document.createElement("div");
    body.className = "section-body";
    body.hidden = collapsed.has(collapseKey);
    toggle.addEventListener("click", () => {
      if (collapsed.has(collapseKey)) {
        collapsed.delete(collapseKey);
      } else {
        collapsed.add(collapseKey);
      }
      body.hidden = collapsed.has(collapseKey);
      toggle.setAttribute("aria-expanded", String(!body.hidden));
    });

    const ctx = contextFor(entry);
    for (const def of settings) {
      if (def.section !== section.id) {
        continue;
      }
      if (def.visibleWhen && !def.visibleWhen(ctx)) {
        continue;
      }
      body.append(fieldRow(entry, def));
    }

    if (section.id === "projector" && store.value(entry, "multimodal_projector") != null) {
      const note = document.createElement("p");
      note.className = "field-help";
      note.textContent = "Images capability is implied by the multimodal projector.";
      body.append(note);
    }

    wrap.append(heading, body);
    return wrap;
  };

  const fieldRow = (entry: ModelEntry, def: SettingDef): HTMLElement => {
    const ctx = contextFor(entry);
    const row = document.createElement("div");
    row.className = "field-row";
    row.dataset["key"] = def.key;
    const id = `field-${def.key.replace(/\./g, "-")}`;

    const head = document.createElement("div");
    head.className = "field-head";
    const label = document.createElement("label");
    label.id = `${id}-label`;
    label.htmlFor = id;
    label.textContent = def.label;
    head.append(label);

    if (store.isEdited(entry, def.key)) {
      const dot = document.createElement("span");
      dot.className = "dirty-dot";
      const hidden = document.createElement("span");
      hidden.className = "visually-hidden";
      hidden.textContent = "edited";
      dot.append(hidden);
      head.append(dot);

      const reset = document.createElement("button");
      reset.type = "button";
      reset.className = "field-reset";
      reset.setAttribute("aria-label", `Reset ${def.label}`);
      reset.append(lucideElement(RotateCcw, { "aria-hidden": "true", width: 12, height: 12 }));
      reset.addEventListener("click", () => store.resetEdit(entry, def.key));
      head.append(reset);
    }

    // Pending chip [INVENTED]: part of the plan's provenance/pending UX.
    const rootKey = def.key.split(".")[0] ?? def.key;
    if (!entry.draft && entry.pendingFields.has(rootKey)) {
      const chip = document.createElement("span");
      chip.className = "pill pill-accent pending-chip";
      chip.textContent = "pending";
      const running = store.runningValue(entry, def.key);
      chip.title =
        running === undefined
          ? "Saved to the shadow; not yet in the running configuration."
          : `Running value: ${JSON.stringify(running)}`;
      head.append(chip);
    }

    row.append(head);

    const disabled =
      def.dependsOn !== undefined &&
      !sameJson(store.value(entry, def.dependsOn.key) ?? defaultOf(entry, def.dependsOn.key), def.dependsOn.value);
    const value = store.value(entry, def.key);

    if (def.type === "slider") {
      const slider = createSliderControl({
        id,
        min: def.min ?? 0,
        max: def.max ?? 100,
        step: def.step ?? 1,
        logScale: def.logScale ?? false,
        maxDetent: def.maxDetent,
        value: typeof value === "number" ? value : Number(def.default ?? def.min ?? 0),
        onChange: (next) => commit(entry, def.key, next),
      });
      if (def.key === "gpu_layers") {
        wireLayerTotal(entry, slider);
      }
      row.append(slider.element);
    } else if (def.type === "toggle") {
      // A configured multimodal projector implies the images capability:
      // the toggle shows on and locks while the projector is present.
      const implied =
        def.key === "images" &&
        entry.kind === "local" &&
        store.value(entry, "multimodal_projector") != null;
      const toggle = createToggleControl({
        id,
        labelledBy: label.id,
        checked: implied || (typeof value === "boolean" ? value : Boolean(def.default)),
        onChange: (next) => commit(entry, def.key, next),
      });
      toggle.setDisabled(disabled || implied);
      row.append(toggle.element);
    } else if (def.type === "dropdown") {
      const options = settingOptions(def, ctx).map((option) => ({
        value: option,
        label: option,
      }));
      if (def.default === null) {
        options.unshift({ value: "", label: "None" });
      }
      const dropdown = createDropdownControl({
        id,
        options,
        value: value == null ? String(def.default ?? "") : String(value),
        onChange: (next) => commit(entry, def.key, next === "" ? null : next),
      });
      dropdown.setDisabled(disabled);
      row.append(dropdown.element);
    } else if (def.type === "chips") {
      const listed = settingOptions(def, ctx);
      const chips = createChipInput({
        id,
        values: Array.isArray(value) ? value.map(String) : [],
        options: listed.length > 0 ? listed : undefined,
        onChange: (next) => commit(entry, def.key, next),
      });
      row.append(chips.element);
    } else {
      let input: HTMLInputElement | HTMLTextAreaElement;
      if (def.type === "textarea") {
        const area = document.createElement("textarea");
        area.rows = 2;
        input = area;
      } else {
        const text = document.createElement("input");
        text.type = "text";
        input = text;
      }
      input.id = id;
      input.className = "input";
      input.disabled = disabled;
      if (def.placeholder) {
        input.placeholder = def.placeholder;
      }
      input.value = value == null ? "" : String(value);
      input.addEventListener("change", () => {
        const text = input.value.trim();
        if (def.numeric) {
          if (text === "") {
            commit(entry, def.key, null);
            return;
          }
          let parsed = Number(text);
          if (Number.isFinite(parsed)) {
            // Declared bounds clamp typed values (default_temperature 0-2).
            if (def.min !== undefined) {
              parsed = Math.max(def.min, parsed);
            }
            if (def.max !== undefined) {
              parsed = Math.min(def.max, parsed);
            }
            commit(entry, def.key, parsed);
          }
          return;
        }
        commit(entry, def.key, text === "" && def.default === null ? null : input.value);
      });
      row.append(input);
    }

    const help = document.createElement("p");
    help.className = "field-help";
    help.id = `${id}-help`;
    help.textContent = def.help;
    row.append(help);
    return row;
  };

  /** The serde default of a dependency key, for entries omitting it. */
  const defaultOf = (entry: ModelEntry, key: string): unknown => {
    const settings = entry.kind === "local" ? LOCAL_MODEL_SETTINGS : REMOTE_MODEL_SETTINGS;
    return settings.find((def) => def.key === key)?.default;
  };

  /** Fetches the GGUF layer count once and feeds the slider readout. */
  const wireLayerTotal = (entry: ModelEntry, slider: SliderControl): void => {
    const known = layerTotals.get(entry.name);
    if (known !== undefined) {
      if (known !== null) {
        slider.setReadoutSuffix(`/ ${known}`);
      }
      return;
    }
    const source = entry.data["source"];
    if (typeof source !== "string" || source === "" || /^[a-z]+:\/\//i.test(source)) {
      layerTotals.set(entry.name, null);
      return;
    }
    layerTotals.set(entry.name, null);
    void api
      .getModelInfo(source)
      .then((info) => {
        layerTotals.set(entry.name, info.layer_count);
        if (info.layer_count === null || selected !== entry.name) {
          return;
        }
        if (slider.element.isConnected) {
          slider.setReadoutSuffix(`/ ${info.layer_count}`);
        } else {
          // The pane re-rendered while the lookup ran; rebuild it so the
          // fresh slider reads the now-cached total.
          renderDetail();
        }
      })
      .catch(() => {
        // Unknown header: the readout stays a plain N.
      });
  };

  // ----- shared bits -------------------------------------------------------

  const statusDot = (name: string): HTMLElement => {
    const dot = document.createElement("span");
    dot.className = "status-dot";
    const running = store.isRunning(name);
    dot.classList.toggle("is-ok", running);
    const hidden = document.createElement("span");
    hidden.className = "visually-hidden";
    hidden.textContent = running ? "running" : "stopped";
    dot.append(hidden);
    return dot;
  };

  const sourceIcon = (kind: "local" | "remote"): HTMLElement => {
    const icon = document.createElement("span");
    icon.className = "source-icon";
    icon.append(
      lucideElement(kind === "local" ? Cpu : Cloud, {
        "aria-hidden": "true",
        width: 14,
        height: 14,
      }),
    );
    const hidden = document.createElement("span");
    hidden.className = "visually-hidden";
    hidden.textContent = kind === "local" ? "local model" : "remote model";
    icon.append(hidden);
    return icon;
  };

  return {
    mount(target: HTMLElement, name?: string): void {
      main = target;
      selected = name;
      render();
    },
  };
}

/** Structural equality through JSON, matching the store's notion. */
function sameJson(a: unknown, b: unknown): boolean {
  return JSON.stringify(a ?? null) === JSON.stringify(b ?? null);
}

/** The last path segment (either separator). */
function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}

/** The quant tag parsed from a GGUF filename, when present. */
function quantFromSource(entry: ModelEntry): string | null {
  const source = entry.data["source"];
  if (typeof source !== "string") {
    return null;
  }
  const base = fileName(source).replace(/\.gguf$/i, "");
  const match = /(?:^|[-._])(i?q\d+(?:_[a-z0-9]+)*|f16|f32|bf16)$/i.exec(base);
  return match?.[1]?.toUpperCase() ?? null;
}

/** Human-readable byte size (GiB/MiB/KiB). */
function formatBytes(bytes: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"] as const;
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}
