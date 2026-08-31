// Slider with click-to-type readout [Unsloth]: a native range input
// paired with a numeric text entry, both driving one value. Supports a
// logarithmic position scale (context-window ranges) and a rightmost
// "Max" detent that maps to a sentinel value (gpu_layers 99999).

/** Construction options for {@link createSliderControl}. */
export interface SliderControlOptions {
  /** The id the field label points at (goes on the range input). */
  id: string;
  /** Range minimum. */
  min: number;
  /** Range maximum (the detent, when present, sits one step past it). */
  max: number;
  /** Range step. */
  step?: number;
  /** Log-scale slider positions. */
  logScale?: boolean;
  /** The value the rightmost detent maps to ("Max"). */
  maxDetent?: number;
  /** The initial value. */
  value: number;
  /** Fired with the committed value on every slider or entry change. */
  onChange: (value: number) => void;
  /** Extra readout text after the value ("/ 32" layer totals). */
  readoutSuffix?: string;
}

/** The mounted slider and its live handles. */
export interface SliderControl {
  /** The row element holding the range input and the readout. */
  element: HTMLElement;
  /** Moves the control to `value` without firing onChange. */
  setValue(value: number): void;
  /** Replaces the readout suffix (when a layer total arrives). */
  setReadoutSuffix(suffix: string): void;
}

/** How many discrete positions a log-scale track exposes. */
const LOG_POSITIONS = 1000;

/** Builds the slider control. */
export function createSliderControl(options: SliderControlOptions): SliderControl {
  const element = document.createElement("div");
  element.className = "slider-row";

  const range = document.createElement("input");
  range.type = "range";
  range.id = options.id;
  range.className = "slider";

  const entry = document.createElement("input");
  entry.type = "text";
  entry.className = "input input-readout";
  entry.inputMode = "numeric";
  entry.setAttribute("aria-label", "Value");

  const suffix = document.createElement("span");
  suffix.className = "readout-suffix";
  suffix.textContent = options.readoutSuffix ?? "";

  const toPosition = (value: number): number => {
    if (options.maxDetent !== undefined && value >= options.maxDetent) {
      return positionMax();
    }
    if (options.logScale) {
      const clamped = Math.min(Math.max(value, options.min), options.max);
      const span = Math.log(options.max) - Math.log(options.min);
      return Math.round(((Math.log(clamped) - Math.log(options.min)) / span) * LOG_POSITIONS);
    }
    return Math.min(Math.max(value, options.min), options.max);
  };

  const fromPosition = (position: number): number => {
    if (options.maxDetent !== undefined && position >= positionMax()) {
      return options.maxDetent;
    }
    if (options.logScale) {
      const span = Math.log(options.max) - Math.log(options.min);
      return Math.round(Math.exp(Math.log(options.min) + (position / LOG_POSITIONS) * span));
    }
    return position;
  };

  /** The track's top position: one past the range when a detent exists. */
  const positionMax = (): number => {
    const top = options.logScale ? LOG_POSITIONS : options.max;
    return options.maxDetent !== undefined ? top + 1 : top;
  };

  range.min = String(options.logScale ? 0 : options.min);
  range.max = String(positionMax());
  range.step = String(options.logScale ? 1 : (options.step ?? 1));

  let current = options.value;

  const renderEntry = (): void => {
    entry.value =
      options.maxDetent !== undefined && current >= options.maxDetent ? "Max" : String(current);
  };

  const setValue = (value: number): void => {
    current = value;
    const position = toPosition(value);
    range.value = String(position);
    range.style.setProperty("--slider-progress", String(position / positionMax()));
    renderEntry();
  };

  // Dragging updates the readout live; the value commits on release
  // ("change"), so owners re-rendering on commit never interrupt a drag.
  range.addEventListener("input", () => {
    current = fromPosition(Number(range.value));
    range.style.setProperty(
      "--slider-progress",
      String(Number(range.value) / positionMax()),
    );
    renderEntry();
  });
  range.addEventListener("change", () => {
    current = fromPosition(Number(range.value));
    range.style.setProperty(
      "--slider-progress",
      String(Number(range.value) / positionMax()),
    );
    renderEntry();
    options.onChange(current);
  });

  entry.addEventListener("change", () => {
    const text = entry.value.trim();
    if (options.maxDetent !== undefined && /^max$/i.test(text)) {
      setValue(options.maxDetent);
      options.onChange(current);
      return;
    }
    const parsed = Number(text);
    if (!Number.isFinite(parsed)) {
      renderEntry();
      return;
    }
    // Typed values clamp to the range unless they name the detent.
    const clamped =
      options.maxDetent !== undefined && parsed >= options.maxDetent
        ? options.maxDetent
        : Math.min(Math.max(Math.round(parsed), options.min), options.max);
    setValue(clamped);
    options.onChange(current);
  });

  setValue(options.value);
  element.append(range, entry, suffix);

  return {
    element,
    setValue,
    setReadoutSuffix(text: string): void {
      suffix.textContent = text;
    },
  };
}
