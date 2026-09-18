// The Run window's contract rows: one renderer per contract section,
// pure DOM building over the narrowed RunContract. Every frontmatter
// key present in the prompt appears as exactly one row: name,
// description, and promptforge read-only; input a text field plus
// Browse; output a text field; one control per arg by type with
// required markers and defaults (the implicit declaration is the single
// prose box); capabilities as checkboxes, disabled when required; tools
// and model roles read-only; max_tool_iterations numeric with the
// runtime-default placeholder. No bind-time validation runs here -
// validation is the parser's alone.

import type { RunContract, RunContractArg } from "../../services/run-api";

/** The panel's hooks the rows need. */
export interface RunRowsOptions {
  /** Fills the input row's field through the panel's file-pick flow. */
  readonly browseInput: (field: HTMLInputElement) => void;
}

/** One labelled row: the label span plus the given control nodes. */
function row(label: string, ...controls: Node[]): HTMLElement {
  const element = document.createElement("div");
  element.className = "ws-run-panel__row";
  const labelElement = document.createElement("span");
  labelElement.className = "ws-run-panel__row-label";
  labelElement.textContent = label;
  element.append(labelElement, ...controls);
  return element;
}

/** A read-only value span. */
function readOnly(text: string): HTMLElement {
  const value = document.createElement("span");
  value.className = "ws-run-panel__value";
  value.textContent = text;
  return value;
}

/** A text field with the shared control skin. */
function textField(value: string, placeholder = ""): HTMLInputElement {
  const field = document.createElement("input");
  field.type = "text";
  field.className = "input ws-run-panel__field";
  field.value = value;
  field.placeholder = placeholder;
  return field;
}

/** The required marker on a non-optional arg. */
function requiredMarker(): HTMLElement {
  const marker = document.createElement("span");
  marker.className = "ws-run-panel__required";
  marker.textContent = "*";
  marker.title = "required";
  return marker;
}

/** One arg's control by its declared type, prefilled from its default. */
function argControl(arg: RunContractArg): HTMLElement {
  if (arg.type === "boolean") {
    const field = document.createElement("input");
    field.type = "checkbox";
    field.className = "ws-run-panel__checkbox";
    field.checked = arg.default === true;
    return field;
  }
  if (arg.type === "integer" || arg.type === "number") {
    const field = document.createElement("input");
    field.type = "number";
    field.className = "input ws-run-panel__field";
    if (arg.type === "integer") {
      field.step = "1";
    }
    if (typeof arg.default === "number") {
      field.value = String(arg.default);
    }
    return field;
  }
  return textField(typeof arg.default === "string" ? arg.default : "");
}

/**
 * Renders every contract section into `container`. The caller clears
 * the container first; rows append in contract order.
 */
export function renderContractRows(
  container: HTMLElement,
  contract: RunContract,
  options: RunRowsOptions,
): void {
  const rows = document.createElement("div");
  rows.className = "ws-run-panel__rows";

  rows.appendChild(row("name", readOnly(contract.name)));
  if (contract.description.length > 0) {
    rows.appendChild(row("description", readOnly(contract.description)));
  }
  if (contract.promptforge !== null) {
    rows.appendChild(row("promptforge", readOnly(String(contract.promptforge))));
  }

  const inputField = textField(contract.input?.path ?? "", contract.input?.description ?? "");
  const inputBrowse = document.createElement("button");
  inputBrowse.type = "button";
  inputBrowse.className = "button button-outline button-sm ws-run-panel__input-browse";
  inputBrowse.textContent = "Browse...";
  inputBrowse.addEventListener("click", () => options.browseInput(inputField));
  rows.appendChild(row("input", inputField, inputBrowse));

  rows.appendChild(
    row("output", textField(contract.output?.path ?? "", contract.output?.description ?? "")),
  );

  if (contract.args.implicit) {
    // The implicit declaration is the single prose field, a multiline box.
    const prose = document.createElement("textarea");
    prose.className = "input ws-run-panel__prose";
    const proseField = contract.args.fields[0];
    if (proseField !== undefined && typeof proseField.default === "string") {
      prose.value = proseField.default;
    }
    rows.appendChild(row(proseField?.name ?? "prose", prose));
  } else {
    for (const arg of contract.args.fields) {
      const control = argControl(arg);
      if (arg.description !== null) {
        control.title = arg.description;
      }
      rows.appendChild(row(arg.name, control, ...(arg.optional ? [] : [requiredMarker()])));
    }
  }

  for (const capability of contract.capabilities) {
    const field = document.createElement("input");
    field.type = "checkbox";
    field.className = "ws-run-panel__checkbox";
    field.checked = true;
    // A required capability cannot be switched off.
    field.disabled = !capability.optional;
    rows.appendChild(row(capability.id, field));
  }

  for (const tool of contract.tools) {
    const toolRow = row(tool.alias, readOnly(`exact: ${tool.path}`));
    toolRow.classList.add("ws-run-panel__row--tool");
    rows.appendChild(toolRow);
  }

  for (const model of contract.models) {
    const details =
      model.minContext === null
        ? model.keywords.join(", ")
        : `${model.keywords.join(", ")} (min context ${model.minContext})`;
    const modelRow = row(model.label, readOnly(details));
    modelRow.classList.add("ws-run-panel__row--model");
    rows.appendChild(modelRow);
  }

  const iterations = document.createElement("input");
  iterations.type = "number";
  iterations.className = "input ws-run-panel__field";
  iterations.step = "1";
  if (contract.maxToolIterations === null) {
    iterations.placeholder = "runtime default";
  } else {
    iterations.value = String(contract.maxToolIterations);
  }
  rows.appendChild(row("max_tool_iterations", iterations));

  container.appendChild(rows);
}
