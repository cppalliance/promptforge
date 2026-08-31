// Chat-template setting control: an Auto / bundled-family / custom-path
// dropdown over the server-owned catalog, plus a read-only summary of the
// effective source, detected family, and reason behind the decision.

import type { ConfigStore, ModelEntry } from "../services/config-store";
import type { ChatTemplateFamily } from "../services/gateway-api";
import { createDropdownControl } from "./dropdown-control";

/** The dropdown value standing in for the custom-path text field. */
const CUSTOM_TEMPLATE_VALUE = "__custom_path__";

/** What the control reads and writes through. */
export interface ChatTemplateControlOptions {
  /** The id the field label points at. */
  id: string;
  /** The model entry being edited. */
  entry: ModelEntry;
  /** The pending-config store backing the field. */
  store: ConfigStore;
  /** Stages one edited field value. */
  commit: (entry: ModelEntry, key: string, value: unknown) => void;
  /** Models whose empty custom-path field is open but not committed yet. */
  customTemplateModes: Set<string>;
}

/** Builds the dropdown, the custom-path field, and the resolution summary. */
export function createChatTemplateControl(options: ChatTemplateControlOptions): HTMLElement {
  const { id, entry, store, commit, customTemplateModes } = options;
  const wrap = document.createElement("div");
  wrap.className = "chat-template-control";
  const raw = store.value(entry, "chat_template_file");
  const configured = typeof raw === "string" ? raw.trim() : "";
  const families = store.chatTemplateFamilies();
  const selectedFamily = families.find((family) => configured === `builtin:${family.slug}`);
  const hasCustomValue = configured !== "" && selectedFamily === undefined;
  const isCustom = hasCustomValue || customTemplateModes.has(entry.name);

  const customField = document.createElement("div");
  customField.className = "chat-template-custom";
  customField.hidden = !isCustom;
  const customLabel = document.createElement("label");
  customLabel.htmlFor = `${id}-custom`;
  customLabel.textContent = "Custom template path";
  const customInput = document.createElement("input");
  customInput.id = `${id}-custom`;
  customInput.className = "input";
  customInput.type = "text";
  customInput.placeholder = "templates/model.jinja";
  customInput.value = hasCustomValue ? configured : "";
  customInput.addEventListener("change", () => {
    const path = customInput.value.trim();
    commit(entry, "chat_template_file", path === "" ? null : path);
  });
  customField.append(customLabel, customInput);

  const dropdown = createDropdownControl({
    id,
    options: [
      { value: "", label: "Auto" },
      ...families.map((family) => ({
        value: `builtin:${family.slug}`,
        label: family.label,
      })),
      { value: CUSTOM_TEMPLATE_VALUE, label: "Custom path" },
    ],
    value: isCustom ? CUSTOM_TEMPLATE_VALUE : (selectedFamily ? configured : ""),
    onChange: (next) => {
      if (next === CUSTOM_TEMPLATE_VALUE) {
        customTemplateModes.add(entry.name);
        customField.hidden = false;
        customInput.focus();
        return;
      }
      customTemplateModes.delete(entry.name);
      customField.hidden = true;
      commit(entry, "chat_template_file", next === "" ? null : next);
    },
  });
  wrap.append(dropdown.element, customField, chatTemplateResolution(store, entry, configured, families));
  return wrap;
}

/** Renders the read-only effective source, detected family, and reason. */
function chatTemplateResolution(
  store: ConfigStore,
  entry: ModelEntry,
  configured: string,
  families: readonly ChatTemplateFamily[],
): HTMLElement {
  const server = store.chatTemplateResolution(entry.name);
  const configuredFamily = families.find((family) => configured === `builtin:${family.slug}`);
  const isCustom = configured !== "" && configuredFamily === undefined;
  const autoEdit = store.isEdited(entry, "chat_template_file") && configured === "";
  const source = configuredFamily
    ? "builtin"
    : isCustom
      ? "custom"
      : autoEdit || server === null
        ? "auto"
        : server.effective_source;
  const effectiveFamily =
    configuredFamily?.slug ?? (isCustom || autoEdit ? null : (server?.effective_family ?? null));
  const reason = configuredFamily
    ? `Built-in ${configuredFamily.label} template is selected.`
    : isCustom
      ? `Custom template path \`${configured}\` is selected.`
      : autoEdit || server === null
        ? "Auto uses a known repair when required, then the GGUF embedded template."
        : server.reason;
  const sourceLabels = {
    auto: "Auto",
    embedded: "Embedded",
    "known-override": "Known override",
    builtin: "Built-in",
    custom: "Custom path",
  } as const;
  const familyLabel = families.find((family) => family.slug === effectiveFamily)?.label;
  const detectedLabel =
    families.find((family) => family.slug === server?.detected_family)?.label ?? "Not detected";

  const details = document.createElement("dl");
  details.className = "chat-template-resolution";
  const add = (term: string, description: string): void => {
    const pair = document.createElement("div");
    const dt = document.createElement("dt");
    dt.textContent = term;
    const dd = document.createElement("dd");
    dd.textContent = description;
    pair.append(dt, dd);
    details.append(pair);
  };
  add("Effective source", `${sourceLabels[source]}${familyLabel ? ` - ${familyLabel}` : ""}`);
  add("Detected family", detectedLabel);
  add("Reason", reason);
  return details;
}
