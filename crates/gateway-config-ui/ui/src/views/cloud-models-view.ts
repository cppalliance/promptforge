// The Cloud tab: the published cloud provider model sheet as a
// browsable catalog. Top to bottom: the kind lozenges as an exclusive
// radio group, the tier-grouped provider dropdown, the family dropdown,
// and the full-height table of canonical entries (the page, not the
// table, scrolls) with a "+N snapshots" disclosure per entry that has
// variants. The add action opens the confirm-details dialog, merges
// into the pending config document, and stages it through
// PUT /admin/config; Apply promotes it. Loading, loaded, and
// unreachable states ride the sheet store's subscription, so a view
// open when the sheet lands re-renders in place.

import type { ToastStack } from "shared-ui/toast";

import {
  canonicalRows,
  displayRule,
  familiesFor,
  providersByTier,
} from "../services/cloud-cascade";
import type { CloudProviderOption } from "../services/cloud-cascade";
import { mergeCloudModel } from "../services/cloud-merge";
import type { ConfigStore } from "../services/config-store";
import type {
  CloudModelEntry,
  CloudProviderSlice,
  GatewayApi,
} from "../services/gateway-api";
import type { SheetStore } from "../services/sheet-store";

/** The kind lozenges: label plus the sheet's `ModelKind` wire value. */
const KINDS: ReadonlyArray<readonly [kind: string, label: string]> = [
  ["chat", "Chat"],
  ["image", "Image"],
  ["video", "Video"],
  ["transcription", "STT"],
  ["speech", "TTS"],
];

/** Construction dependencies for the Cloud view. */
export interface CloudModelsViewDeps {
  /** The config store: the payload base and the staging write path. */
  store: ConfigStore;
  /** The cloud sheet store driving the loading/loaded/error states. */
  sheets: SheetStore;
  /** The admin API (unused directly; the stores wrap it). */
  api: GatewayApi;
  /** Outcome surfacing. */
  toasts: ToastStack;
}

/** The mounted view. */
export interface CloudModelsView {
  /** Renders the view into `main`; returns the unmount cleanup. */
  mount(main: HTMLElement): () => void;
}

/** Builds the Cloud view. */
export function createCloudModelsView(deps: CloudModelsViewDeps): CloudModelsView {
  const { store, sheets, toasts } = deps;

  let kind = "chat";
  let provider: string | null = null;
  let family: string | null = null;
  /** Canonical entry ids whose snapshot rows are expanded. */
  const expanded = new Set<string>();
  let main: HTMLElement | null = null;
  let viewRoot: HTMLElement | null = null;

  /** The currently selected provider option, when one is selected. */
  const selectedProvider = (): CloudProviderOption | null => {
    const sheet = sheets.sheet;
    if (!sheet) {
      return null;
    }
    const groups = providersByTier(sheet, kind);
    const flat = groups.flatMap((group) => group.providers);
    const current = flat.find((option) => option.name === provider);
    if (current && !current.disabled) {
      return current;
    }
    // The selection is stale (first render, or a kind switch greyed
    // it): fall to the first provider that serves the kind.
    const fallback = flat.find((option) => !option.disabled) ?? flat[0] ?? null;
    provider = fallback?.name ?? null;
    return fallback;
  };

  const render = (): void => {
    if (!main) {
      return;
    }
    const title = document.createElement("h1");
    title.className = "view-title";
    title.textContent = "Cloud";

    const root = document.createElement("div");
    root.className = "cloud-view";
    viewRoot = root;

    if (sheets.status === "error" && sheets.sheet === null) {
      const failed = document.createElement("p");
      failed.className = "view-empty";
      failed.textContent = sheets.error ?? "The cloud model sheet is unreachable.";
      const retry = document.createElement("button");
      retry.type = "button";
      retry.className = "button button-outline cloud-retry";
      retry.textContent = "Retry";
      retry.addEventListener("click", () => sheets.start());
      root.append(failed, retry);
      main.replaceChildren(title, root);
      return;
    }
    if (sheets.sheet === null) {
      const loading = document.createElement("p");
      loading.className = "view-empty";
      loading.textContent = "Loading the cloud model sheet…";
      root.append(loading);
      main.replaceChildren(title, root);
      return;
    }

    root.append(controls(), table());
    main.replaceChildren(title, root);
  };

  /** The lozenge row, the two dropdowns, and the generated-at note. */
  const controls = (): HTMLElement => {
    const sheet = sheets.sheet;
    const wrap = document.createElement("div");
    wrap.className = "cloud-controls";

    const lozenges = document.createElement("div");
    lozenges.className = "filter-chips cloud-kinds";
    lozenges.setAttribute("role", "radiogroup");
    lozenges.setAttribute("aria-label", "Model kind");
    for (const [value, label] of KINDS) {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.className = "pill filter-chip cloud-kind";
      chip.dataset["kind"] = value;
      chip.setAttribute("role", "radio");
      chip.setAttribute("aria-checked", String(value === kind));
      chip.textContent = label;
      chip.addEventListener("click", () => {
        if (kind !== value) {
          kind = value;
          family = null;
          render();
        }
      });
      lozenges.append(chip);
    }
    wrap.append(lozenges);

    const groups = sheet ? providersByTier(sheet, kind) : [];
    const providerSelect = document.createElement("select");
    providerSelect.className = "select cloud-provider-select";
    const providerLabel = document.createElement("label");
    providerLabel.className = "visually-hidden";
    providerLabel.textContent = "Provider";
    providerLabel.htmlFor = "cloud-provider";
    providerSelect.id = "cloud-provider";
    for (const group of groups) {
      const optgroup = document.createElement("optgroup");
      optgroup.label = group.tier[0]?.toUpperCase() + group.tier.slice(1);
      for (const option of group.providers) {
        const item = document.createElement("option");
        item.value = option.name;
        item.textContent = option.displayName;
        item.disabled = option.disabled;
        optgroup.append(item);
      }
      providerSelect.append(optgroup);
    }
    const selected = selectedProvider();
    if (selected) {
      providerSelect.value = selected.name;
    }
    providerSelect.addEventListener("change", () => {
      provider = providerSelect.value;
      family = null;
      render();
    });
    wrap.append(providerLabel, providerSelect);

    const familySelect = document.createElement("select");
    familySelect.className = "select cloud-family-select";
    const familyLabel = document.createElement("label");
    familyLabel.className = "visually-hidden";
    familyLabel.textContent = "Family";
    familyLabel.htmlFor = "cloud-family";
    familySelect.id = "cloud-family";
    const all = document.createElement("option");
    all.value = "";
    all.textContent = "All families";
    familySelect.append(all);
    for (const name of selected ? familiesFor(selected.slice, kind) : []) {
      const item = document.createElement("option");
      item.value = name;
      item.textContent = name;
      familySelect.append(item);
    }
    familySelect.value = family ?? "";
    familySelect.addEventListener("change", () => {
      family = familySelect.value === "" ? null : familySelect.value;
      render();
    });
    wrap.append(familyLabel, familySelect);

    const meta = document.createElement("span");
    meta.className = "field-help cloud-generated";
    meta.textContent = `Sheet generated ${sheet?.generated_at ?? ""}`;
    const refresh = document.createElement("button");
    refresh.type = "button";
    refresh.className = "button button-xs button-outline cloud-refresh";
    refresh.textContent = "Refresh";
    refresh.addEventListener("click", () => {
      void sheets.refresh().catch((error: unknown) => {
        toasts.show(error instanceof Error ? error.message : "The refresh failed", "error");
      });
    });
    wrap.append(meta, refresh);
    return wrap;
  };

  /** The capability pills for one entry: tools, images, thinking. */
  const capabilities = (entry: CloudModelEntry): HTMLElement => {
    const cell = document.createElement("td");
    cell.className = "cloud-caps";
    const caps: string[] = [];
    if (entry.tool_calling) {
      caps.push("tools");
    }
    if (entry.images) {
      caps.push("images");
    }
    if (entry.thinking.supported) {
      caps.push("thinking");
    }
    for (const cap of caps) {
      const pill = document.createElement("span");
      pill.className = "pill capability-pill";
      pill.textContent = cap;
      cell.append(pill);
    }
    if (caps.length === 0) {
      cell.textContent = "-";
    }
    return cell;
  };

  /** One table row (canonical or variant). */
  const modelRow = (
    option: CloudProviderOption,
    entry: CloudModelEntry,
    variants: CloudModelEntry[],
    isVariant: boolean,
  ): HTMLTableRowElement => {
    const row = document.createElement("tr");
    row.className = isVariant ? "cloud-variant-row" : "cloud-row";
    row.dataset["id"] = entry.id;

    const name = document.createElement("td");
    name.className = "cloud-name";
    const display = displayRule(entry);
    const primary = document.createElement("span");
    primary.className = "cloud-name-primary";
    primary.textContent = isVariant && entry.variant !== null ? entry.variant : display.primary;
    name.append(primary);
    if (display.secondary !== null) {
      const id = document.createElement("code");
      id.className = "cloud-name-id";
      id.textContent = display.secondary;
      name.append(id);
    }
    if (!isVariant && variants.length > 0) {
      const toggle = document.createElement("button");
      toggle.type = "button";
      toggle.className = "button button-xs button-outline cloud-variants-toggle";
      toggle.textContent = `+${variants.length} snapshots`;
      toggle.setAttribute("aria-expanded", String(expanded.has(entry.id)));
      toggle.addEventListener("click", () => {
        if (expanded.has(entry.id)) {
          expanded.delete(entry.id);
        } else {
          expanded.add(entry.id);
        }
        render();
      });
      name.append(toggle);
    }
    row.append(name);

    const context = document.createElement("td");
    context.className = "cloud-context";
    context.textContent =
      entry.context_window !== null ? entry.context_window.toLocaleString("en-US") : "-";
    row.append(context);

    const maxOutput = document.createElement("td");
    maxOutput.className = "cloud-max-output";
    maxOutput.textContent =
      entry.max_output !== null ? entry.max_output.toLocaleString("en-US") : "-";
    row.append(maxOutput);

    row.append(capabilities(entry));

    const pricing = document.createElement("td");
    pricing.className = "cloud-pricing";
    pricing.textContent = entry.pricing
      ? `${entry.pricing.currency} ${entry.pricing.prompt_per_mtok} / ${entry.pricing.completion_per_mtok} per Mtok`
      : "-";
    row.append(pricing);

    const action = document.createElement("td");
    action.className = "cloud-action";
    const add = document.createElement("button");
    add.type = "button";
    add.className = "button button-xs button-outline cloud-add";
    add.textContent = "Add";
    if (option.slice.openai_base_url === null) {
      add.disabled = true;
      const reason = document.createElement("span");
      reason.className = "cloud-no-add";
      reason.textContent = "no OpenAI-compatible endpoint";
      action.append(add, reason);
    } else {
      add.addEventListener("click", () => openAddDialog(option.name, option.slice, entry));
      action.append(add);
    }
    row.append(action);
    return row;
  };

  /** The full-height catalog table for the selected provider. */
  const table = (): HTMLElement => {
    const selected = selectedProvider();
    const table = document.createElement("table");
    table.className = "cloud-table";
    const thead = document.createElement("thead");
    const headRow = document.createElement("tr");
    for (const label of ["Name", "Context", "Max output", "Capabilities", "Pricing"]) {
      const th = document.createElement("th");
      th.scope = "col";
      th.textContent = label;
      headRow.append(th);
    }
    const actionsHead = document.createElement("th");
    actionsHead.scope = "col";
    const actionsLabel = document.createElement("span");
    actionsLabel.className = "visually-hidden";
    actionsLabel.textContent = "Actions";
    actionsHead.append(actionsLabel);
    headRow.append(actionsHead);
    thead.append(headRow);
    table.append(thead);

    const tbody = document.createElement("tbody");
    if (selected) {
      for (const { entry, variants } of canonicalRows(selected.slice, kind, family)) {
        tbody.append(modelRow(selected, entry, variants, false));
        if (expanded.has(entry.id)) {
          for (const variant of variants) {
            tbody.append(modelRow(selected, variant, [], true));
          }
        }
      }
    }
    table.append(tbody);

    const wrap = document.createElement("div");
    wrap.className = "cloud-table-wrap";
    wrap.append(table);
    if (!selected || tbody.childElementCount === 0) {
      const empty = document.createElement("p");
      empty.className = "view-empty";
      empty.textContent = "No models of this kind.";
      wrap.append(empty);
    }
    return wrap;
  };

  /**
   * The confirm-details dialog: pre-filled from the sheet entry (name
   * defaults to the display name and stays editable, context pre-fills
   * from the sheet and is required when the sheet lacks it, description
   * shows and edits). Submit merges and stages via PUT /admin/config;
   * shadow-save validation errors surface verbatim and the dialog stays
   * open. Hosted in `main`; a re-render replaces it.
   */
  const openAddDialog = (
    providerName: string,
    slice: CloudProviderSlice,
    entry: CloudModelEntry,
  ): void => {
    if (!main || main.querySelector(".cloud-add-overlay") !== null) {
      return;
    }
    const overlay = document.createElement("div");
    overlay.className = "modal-overlay cloud-add-overlay";
    const dialog = document.createElement("section");
    dialog.className = "modal-dialog cloud-add";
    dialog.setAttribute("role", "dialog");
    dialog.setAttribute("aria-modal", "true");
    dialog.setAttribute("aria-labelledby", "cloud-add-title");

    const title = document.createElement("h2");
    title.id = "cloud-add-title";
    title.className = "cloud-add__title";
    title.textContent = `Add ${entry.display_name}`;

    const close = (): void => {
      document.removeEventListener("keydown", onKeydown, true);
      overlay.remove();
    };
    const field = (
      labelText: string,
      className: string,
      value: string,
    ): { wrap: HTMLElement; input: HTMLInputElement } => {
      const wrap = document.createElement("div");
      wrap.className = "cloud-add__field";
      const label = document.createElement("label");
      label.className = "cloud-add__label";
      label.textContent = labelText;
      const input = document.createElement("input");
      input.type = "text";
      input.className = `input ${className}`;
      input.value = value;
      label.append(input);
      wrap.append(label);
      return { wrap, input };
    };

    const name = field("Name", "cloud-add-name", entry.display_name);
    const contextRequired = entry.context_window === null;
    const context = field(
      contextRequired ? "Context (required - not reported by the provider)" : "Context",
      "cloud-add-context",
      entry.context_window !== null ? String(entry.context_window) : "",
    );
    const description = field("Description", "cloud-add-description", entry.display_name);

    const error = document.createElement("p");
    error.className = "cloud-add-error";
    error.setAttribute("role", "alert");
    error.hidden = true;
    const fail = (message: string): void => {
      error.textContent = message;
      error.hidden = false;
    };

    const actions = document.createElement("div");
    actions.className = "modal-actions cloud-add__actions";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "button button-outline";
    cancel.textContent = "Cancel";
    cancel.addEventListener("click", close);
    const submit = document.createElement("button");
    submit.type = "button";
    submit.className = "button button-primary cloud-add-submit";
    submit.textContent = "Add Model";
    submit.addEventListener("click", () => {
      const modelName = name.input.value.trim();
      if (modelName === "") {
        fail("Name is required");
        return;
      }
      let contextValue: number | undefined;
      const raw = context.input.value.trim();
      if (raw !== "") {
        const parsed = Number(raw);
        if (!Number.isInteger(parsed) || parsed <= 0) {
          fail("Context must be a positive integer");
          return;
        }
        contextValue = parsed;
      }
      const descriptionValue = description.input.value.trim();
      void (async () => {
        try {
          const payload = store.buildConfigPayload();
          mergeCloudModel(payload, providerName, slice, entry, {
            name: modelName,
            context: contextValue,
            description: descriptionValue === "" ? undefined : descriptionValue,
          });
          await store.savePayload(payload);
          toasts.show(`${modelName} added - Apply to activate`, "success");
          close();
        } catch (caught) {
          fail(caught instanceof Error ? caught.message : String(caught));
        }
      })();
    });
    actions.append(cancel, submit);

    dialog.append(title, name.wrap, context.wrap, description.wrap, error, actions);
    overlay.append(dialog);
    overlay.addEventListener("mousedown", (event) => {
      if (event.target === overlay) {
        close();
      }
    });
    const onKeydown = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        close();
      }
    };
    document.addEventListener("keydown", onKeydown, true);
    main.append(overlay);
    name.input.focus();
  };

  return {
    mount(target: HTMLElement): () => void {
      main = target;
      const unsubscribe = sheets.subscribe(() => {
        if (main?.isConnected && viewRoot?.isConnected) {
          render();
        }
      });
      render();
      return () => {
        unsubscribe();
        main = null;
        viewRoot = null;
      };
    },
  };
}
