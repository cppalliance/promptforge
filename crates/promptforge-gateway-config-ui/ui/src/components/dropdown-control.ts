// Styled select [Unsloth]: a native <select> carrying the .select pill
// styling; an optional empty option renders "None"-style defaults.

/** One option row. */
export interface DropdownOption {
  /** The stored value ("" for the empty/None row). */
  value: string;
  /** The visible text. */
  label: string;
}

/** Construction options for {@link createDropdownControl}. */
export interface DropdownControlOptions {
  /** The id the field label points at. */
  id: string;
  /** The option rows, in order. */
  options: DropdownOption[];
  /** The initial value. */
  value: string;
  /** Fired with the new value on change. */
  onChange: (value: string) => void;
}

/** The mounted select and its live handles. */
export interface DropdownControl {
  /** The select element. */
  element: HTMLSelectElement;
  /** Moves the selection without firing onChange. */
  setValue(value: string): void;
  /** Enables or disables the select. */
  setDisabled(disabled: boolean): void;
}

/** Builds the select. */
export function createDropdownControl(options: DropdownControlOptions): DropdownControl {
  const element = document.createElement("select");
  element.id = options.id;
  element.className = "select";
  for (const option of options.options) {
    const row = document.createElement("option");
    row.value = option.value;
    row.textContent = option.label;
    element.append(row);
  }
  element.value = options.value;

  element.addEventListener("change", () => options.onChange(element.value));

  return {
    element,
    setValue(value: string): void {
      element.value = value;
    },
    setDisabled(disabled: boolean): void {
      element.disabled = disabled;
    },
  };
}
