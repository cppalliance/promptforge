// Chip/tag entry [Adapted: Open WebUI]: a wrapping soft field of
// removable chips plus a bare inline text entry - Enter adds, the X (or
// Backspace on an empty entry) removes. When `options` is set the input
// only accepts listed values (the endpoints multi-select), offered
// through a datalist.

import { X, createElement as lucideElement } from "lucide";

/** Construction options for {@link createChipInput}. */
export interface ChipInputOptions {
  /** The id the field label points at (goes on the inner entry). */
  id: string;
  /** The initial chips. */
  values: string[];
  /** When set, only these values are accepted (and suggested). */
  options?: string[];
  /** Fired with the full chip list after every add or remove. */
  onChange: (values: string[]) => void;
}

/** The mounted chip input and its live handles. */
export interface ChipInput {
  /** The wrapping field element. */
  element: HTMLElement;
  /** Replaces the chips without firing onChange. */
  setValues(values: string[]): void;
}

/** Builds the chip input. */
export function createChipInput(options: ChipInputOptions): ChipInput {
  const element = document.createElement("div");
  element.className = "chip-input";

  const entry = document.createElement("input");
  entry.type = "text";
  entry.id = options.id;

  let values = [...options.values];

  if (options.options) {
    const list = document.createElement("datalist");
    list.id = `${options.id}-options`;
    for (const value of options.options) {
      const row = document.createElement("option");
      row.value = value;
      list.append(row);
    }
    entry.setAttribute("list", list.id);
    element.append(list);
  }

  const render = (): void => {
    for (const chip of element.querySelectorAll(".pill")) {
      chip.remove();
    }
    for (const value of values) {
      const chip = document.createElement("span");
      chip.className = "pill";
      const text = document.createElement("span");
      text.textContent = value;
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "chip-remove";
      remove.setAttribute("aria-label", `Remove ${value}`);
      remove.append(lucideElement(X, { "aria-hidden": "true", width: 12, height: 12 }));
      remove.addEventListener("click", () => {
        values = values.filter((existing) => existing !== value);
        render();
        options.onChange([...values]);
      });
      chip.append(text, remove);
      element.insertBefore(chip, entry);
    }
  };

  const add = (raw: string): void => {
    const value = raw.trim();
    if (value === "" || values.includes(value)) {
      return;
    }
    if (options.options && !options.options.includes(value)) {
      return;
    }
    values.push(value);
    entry.value = "";
    render();
    options.onChange([...values]);
  };

  entry.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      add(entry.value);
    } else if (event.key === "Backspace" && entry.value === "" && values.length > 0) {
      values = values.slice(0, -1);
      render();
      options.onChange([...values]);
    }
  });

  element.append(entry);
  render();

  return {
    element,
    setValues(next: string[]): void {
      values = [...next];
      render();
    },
  };
}
