// On/off switch [Unsloth]: a <button role="switch"> styled by the
// .switch component class; aria-checked is the state.

/** Construction options for {@link createToggleControl}. */
export interface ToggleControlOptions {
  /** The id the field label points at. */
  id: string;
  /** The element id of the label naming this switch. */
  labelledBy: string;
  /** The initial state. */
  checked: boolean;
  /** Fired with the new state on every toggle. */
  onChange: (checked: boolean) => void;
}

/** The mounted switch and its live handles. */
export interface ToggleControl {
  /** The switch button. */
  element: HTMLButtonElement;
  /** Moves the switch without firing onChange. */
  setChecked(checked: boolean): void;
  /** Enables or disables the switch. */
  setDisabled(disabled: boolean): void;
}

/** Builds the switch. */
export function createToggleControl(options: ToggleControlOptions): ToggleControl {
  const element = document.createElement("button");
  element.type = "button";
  element.id = options.id;
  element.className = "switch";
  element.setAttribute("role", "switch");
  element.setAttribute("aria-checked", String(options.checked));
  element.setAttribute("aria-labelledby", options.labelledBy);

  element.addEventListener("click", () => {
    const next = element.getAttribute("aria-checked") !== "true";
    element.setAttribute("aria-checked", String(next));
    options.onChange(next);
  });

  return {
    element,
    setChecked(checked: boolean): void {
      element.setAttribute("aria-checked", String(checked));
    },
    setDisabled(disabled: boolean): void {
      element.disabled = disabled;
    },
  };
}
