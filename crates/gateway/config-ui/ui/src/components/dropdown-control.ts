// Styled disclosure select [Unsloth]: a button trigger opens an ARIA
// listbox built from the shared menu surface and menu-item rows. Rows
// may be greyed (`disabled`: skipped by navigation and typeahead, never
// chosen) and grouped (`group`: a presentational header renders before
// the first row of each run sharing a group name).

/** One option row. */
export interface DropdownOption {
  /** The stored value ("" for the empty/None row). */
  value: string;
  /** The visible text. */
  label: string;
  /** Greyed and unchoosable; keyboard navigation skips it. */
  disabled?: boolean;
  /** The header shown above this row when it differs from the previous row's. */
  group?: string;
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
  /** The wrapper holding the trigger and listbox. */
  element: HTMLElement;
  /** The disclosure trigger associated with the field label. */
  trigger: HTMLButtonElement;
  /** Moves the selection without firing onChange. */
  setValue(value: string): void;
  /** Enables or disables the select. */
  setDisabled(disabled: boolean): void;
}

/** Builds the disclosure and its keyboard-operable listbox. */
export function createDropdownControl(options: DropdownControlOptions): DropdownControl {
  const element = document.createElement("div");
  element.className = "dropdown-control";

  const trigger = document.createElement("button");
  trigger.type = "button";
  trigger.id = options.id;
  trigger.className = "select";
  trigger.setAttribute("aria-haspopup", "listbox");
  trigger.setAttribute("aria-expanded", "false");

  const menu = document.createElement("div");
  menu.id = `${options.id}-listbox`;
  menu.className = "menu dropdown-menu";
  menu.setAttribute("role", "listbox");
  menu.setAttribute("aria-labelledby", options.id);
  menu.hidden = true;
  trigger.setAttribute("aria-controls", menu.id);

  const rows: HTMLButtonElement[] = [];
  let current = options.value;
  let typeahead = "";
  let typeaheadTimer: ReturnType<typeof setTimeout> | null = null;

  const enabled = (index: number): boolean => options.options[index]?.disabled !== true;

  /** The first enabled index at or after `from` (wrapping), or -1. */
  const enabledFrom = (from: number, delta: 1 | -1): number => {
    const count = rows.length;
    for (let step = 0; step < count; step += 1) {
      const index = (((from + delta * step) % count) + count) % count;
      if (enabled(index)) {
        return index;
      }
    }
    return -1;
  };

  /** The selected row's index, or the first enabled row when it is greyed or absent. */
  const currentIndex = (): number => {
    const index = options.options.findIndex((option) => option.value === current);
    if (index >= 0 && enabled(index)) {
      return index;
    }
    return Math.max(enabledFrom(0, 1), 0);
  };

  const renderValue = (): void => {
    const selected = options.options.find((option) => option.value === current);
    trigger.value = current;
    trigger.textContent = selected?.label ?? current;
    rows.forEach((row, index) => {
      row.setAttribute("aria-selected", String(options.options[index]?.value === current));
    });
  };

  const close = (restoreFocus = false): void => {
    menu.hidden = true;
    trigger.setAttribute("aria-expanded", "false");
    document.removeEventListener("pointerdown", onDocumentPointerDown);
    if (restoreFocus) {
      trigger.focus();
    }
  };

  const focusIndex = (index: number): void => {
    rows[Math.min(Math.max(index, 0), rows.length - 1)]?.focus();
  };

  const open = (focusAt = currentIndex()): void => {
    if (trigger.disabled || rows.length === 0) {
      return;
    }
    menu.hidden = false;
    trigger.setAttribute("aria-expanded", "true");
    document.addEventListener("pointerdown", onDocumentPointerDown);
    focusIndex(focusAt);
  };

  const choose = (index: number): void => {
    const option = options.options[index];
    if (!option || option.disabled === true) {
      return;
    }
    current = option.value;
    renderValue();
    close(true);
    options.onChange(current);
  };

  const onDocumentPointerDown = (event: Event): void => {
    if (!element.contains(event.target as Node)) {
      close();
    }
  };

  const moveFocus = (event: KeyboardEvent, index: number): void => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      focusIndex(enabledFrom(index + 1, 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      focusIndex(enabledFrom(index - 1, -1));
    } else if (event.key === "Home") {
      event.preventDefault();
      focusIndex(enabledFrom(0, 1));
    } else if (event.key === "End") {
      event.preventDefault();
      focusIndex(enabledFrom(rows.length - 1, -1));
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      choose(index);
    } else if (event.key === "Escape") {
      event.preventDefault();
      close(true);
    } else if (event.key === "Tab") {
      close();
    } else if (event.key.length === 1 && /\S/.test(event.key)) {
      typeahead += event.key.toLocaleLowerCase();
      if (typeaheadTimer !== null) {
        clearTimeout(typeaheadTimer);
      }
      typeaheadTimer = setTimeout(() => {
        typeahead = "";
        typeaheadTimer = null;
      }, 500);
      const match = options.options.findIndex(
        (option) =>
          option.disabled !== true && option.label.toLocaleLowerCase().startsWith(typeahead),
      );
      if (match >= 0) {
        focusIndex(match);
      }
    }
  };

  options.options.forEach((option, index) => {
    if (option.group !== undefined && option.group !== options.options[index - 1]?.group) {
      const header = document.createElement("div");
      header.className = "menu-group-label";
      header.setAttribute("role", "presentation");
      header.textContent = option.group;
      menu.append(header);
    }
    const row = document.createElement("button");
    row.type = "button";
    row.id = `${menu.id}-option-${index}`;
    row.className = "menu-item";
    row.dataset["value"] = option.value;
    row.setAttribute("role", "option");
    row.tabIndex = -1;
    row.textContent = option.label;
    if (option.disabled === true) {
      row.disabled = true;
      row.setAttribute("aria-disabled", "true");
    }
    row.addEventListener("click", () => choose(index));
    row.addEventListener("keydown", (event) => moveFocus(event, index));
    rows.push(row);
    menu.append(row);
  });

  trigger.addEventListener("click", () => {
    if (menu.hidden) {
      open();
    } else {
      close();
    }
  });
  trigger.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Home" || event.key === "End") {
      event.preventDefault();
      if (event.key === "ArrowUp" || event.key === "End") {
        open(enabledFrom(rows.length - 1, -1));
      } else if (event.key === "Home") {
        open(enabledFrom(0, 1));
      } else {
        open();
      }
    } else if (event.key === "Escape" && !menu.hidden) {
      event.preventDefault();
      close(true);
    }
  });

  renderValue();
  element.append(trigger, menu);

  return {
    element,
    trigger,
    setValue(value: string): void {
      current = value;
      renderValue();
    },
    setDisabled(disabled: boolean): void {
      trigger.disabled = disabled;
      if (disabled) {
        close();
      }
    },
  };
}
