// The transcript's row components: one class per row kind of the model
// (transcript-model.ts), each keeping its own DOM and its own open or
// expanded state between `update(row)` calls. The view reconciles rows by
// key, reusing the component for a key and calling `update` with the
// freshly built row, so a streaming delta changes the text in place and
// never rebuilds a row the operator is reading or has opened.
//
// - MarkdownRowView: an assistant reply, no label and no bubble.
// - ThoughtRowView: thinking and nothing else, a collapsible.
// - GroupRowView: thinking and tool steps that ran together, a
//   collapsible whose steps sit in a muted scope; the tail status can
//   render inside it.
// - ToolRowView: one standalone tool line.
// - HumanMessage (human-message.ts) is the operator's row.
//
// Reply and thinking text is model-authored markdown and goes through
// renderMarkdown (DOMPurify is its last step); everything else lands as
// text.

import { renderMarkdown, MarkdownStream } from "../markdown-render";
import type { IDisposable } from "@workshop/platform/lifecycle";
import { Collapsible, type ShimmerTone } from "./collapsible";
import { HumanMessage } from "./human-message";
import { reconcile, type KeyedComponent } from "./reconcile";
import { ToolLine } from "./tool-line";
import type {
  GroupRow,
  GroupStep,
  MarkdownRow,
  ThoughtLabel,
  ThoughtRow,
  ThoughtStep,
  ToolRow,
  ToolStep,
  TranscriptRow,
} from "./transcript-model";

// --- Assistant reply ------------------------------------------------------------------

/** An assistant reply: markdown, fading new text in while it streams. */
export class MarkdownRowView implements IDisposable {
  readonly kind = "markdown";
  readonly element: HTMLDivElement;

  private readonly stream = new MarkdownStream();
  private source: string | null = null;
  private streaming = false;

  /** `animateIn` fades a new row in and lifts it 8px; the first paint of a feed skips it. */
  constructor(animateIn: boolean) {
    this.element = document.createElement("div");
    this.element.className = "ws-markdown-row";
    if (animateIn) {
      this.element.classList.add("ws-markdown-row--enter");
    }
  }

  update(row: MarkdownRow): void {
    if (row.text === this.source && row.streaming === this.streaming) {
      return;
    }
    this.source = row.text;
    this.streaming = row.streaming;
    this.element.dataset["streaming"] = String(row.streaming);
    this.element.replaceChildren(this.stream.render(row.text, { streaming: row.streaming }));
  }

  copyText(): string {
    return this.source ?? "";
  }

  dispose(): void {
    // Nothing is held beyond the element.
  }
}

// --- Thinking -------------------------------------------------------------------------

/** What a thinking line reads. */
interface ThinkingInput {
  readonly label: ThoughtLabel;
  readonly text: string;
  readonly streaming: boolean;
}

/**
 * Thinking as a collapsible: the label in the header (shimmering while it
 * streams) over the thinking markdown at half opacity. `openWhileStreaming`
 * is the group's rule - its thinking lines open while they stream and close
 * once settled - where a thought row stays closed unless the operator opens
 * it. Either way the operator's own click wins and survives every update.
 * The body renders only while open: a closed line keeps its latest text and
 * paints it when opened.
 */
class ThinkingLine implements IDisposable {
  readonly element: HTMLElement;

  private readonly collapsible: Collapsible;
  private readonly content: HTMLDivElement;
  private text = "";
  private painted: string | null = null;

  constructor(private readonly openWhileStreaming: boolean, className: string) {
    this.collapsible = new Collapsible({
      onToggle: (open) => {
        if (open) {
          this.paint();
        }
      },
    });
    this.element = this.collapsible.element;
    this.element.classList.add(className);
    this.content = document.createElement("div");
    this.content.className = "ws-thinking-body";
    this.collapsible.body.appendChild(this.content);
  }

  update(input: ThinkingInput): void {
    this.text = input.text;
    const tone: ShimmerTone | null = input.streaming ? "thinking" : null;
    this.collapsible.setHeader({
      action: input.label.action,
      details: input.label.details,
      shimmer: tone,
    });
    this.collapsible.setExpandable(input.text.trim() !== "");
    this.collapsible.setDefaultOpen(this.openWhileStreaming && input.streaming);
    if (this.collapsible.open) {
      this.paint();
    }
  }

  copyText(): string {
    return this.text;
  }

  get operatorChoice(): boolean | null {
    return this.collapsible.operatorChoice;
  }

  adoptChoice(choice: boolean | null): void {
    this.collapsible.adoptChoice(choice);
  }

  dispose(): void {
    // Nothing is held beyond the element.
  }

  private paint(): void {
    if (this.painted === this.text) {
      return;
    }
    this.painted = this.text;
    this.content.replaceChildren(renderMarkdown(this.text));
  }
}

/** Thinking and nothing else: a closed-by-default collapsible. */
export class ThoughtRowView implements IDisposable {
  readonly kind = "thought";
  readonly element: HTMLElement;

  private readonly line = new ThinkingLine(false, "ws-thought-row");

  constructor() {
    this.element = this.line.element;
  }

  update(row: ThoughtRow): void {
    this.line.update({ label: row.label, text: row.text, streaming: row.streaming });
  }

  copyText(): string {
    return this.line.copyText();
  }

  /** The operator's own open or closed choice, or null while they've made none. */
  get operatorChoice(): boolean | null {
    return this.line.operatorChoice;
  }

  dispose(): void {
    this.line.dispose();
  }
}

// --- Tools ----------------------------------------------------------------------------

/** One standalone tool line. */
export class ToolRowView implements IDisposable {
  readonly kind = "tool";
  readonly element: HTMLElement;

  private readonly line = new ToolLine();

  constructor() {
    this.element = this.line.element;
  }

  update(row: ToolRow): void {
    this.line.update(row.step);
  }

  copyText(): string {
    return this.line.copyText();
  }

  /** The operator's own open or closed choice, or null while they've made none. */
  get operatorChoice(): boolean | null {
    return this.line.operatorChoice;
  }

  dispose(): void {
    this.line.dispose();
  }
}

// --- Group ----------------------------------------------------------------------------

/** A group's thinking step: an inner thinking line that opens while it streams. */
class ThinkingStepView implements KeyedComponent {
  readonly kind = "thought";
  readonly element: HTMLElement;

  private readonly line = new ThinkingLine(true, "ws-thinking-line");

  constructor() {
    this.element = this.line.element;
  }

  update(step: ThoughtStep): void {
    this.line.update({ label: step.label, text: step.text, streaming: step.streaming });
  }

  adoptChoice(choice: boolean | null): void {
    this.line.adoptChoice(choice);
  }

  dispose(): void {
    this.line.dispose();
  }
}

/** A group's tool step. */
class ToolStepView implements KeyedComponent {
  readonly kind = "tool";
  readonly element: HTMLElement;

  private readonly line = new ToolLine();

  constructor() {
    this.element = this.line.element;
  }

  update(step: ToolStep): void {
    this.line.update(step);
  }

  adoptChoice(choice: boolean | null): void {
    this.line.adoptChoice(choice);
  }

  dispose(): void {
    this.line.dispose();
  }
}

/** A step component inside a group. */
type StepComponent = ThinkingStepView | ToolStepView;

/** The group's steps as reconcile entries. */
interface StepEntry {
  readonly key: string;
  readonly kind: "thought" | "tool";
  readonly step: GroupStep;
}

/** Thinking and tool steps that ran together. */
export class GroupRowView implements IDisposable {
  readonly kind = "group";
  readonly element: HTMLDivElement;

  private readonly collapsible = new Collapsible();
  private readonly steps = new Map<string, StepComponent>();
  private readonly list: HTMLDivElement;
  private text = "";

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "ws-group-row";
    this.element.appendChild(this.collapsible.element);
    this.list = document.createElement("div");
    this.list.className = "ws-group-row__steps ws-muted-scope";
    this.collapsible.body.appendChild(this.list);
  }

  update(row: GroupRow): void {
    this.collapsible.setHeader({ action: row.summary.action, details: row.summary.details });
    const entries: StepEntry[] = row.steps.map((step) => ({
      key: step.key,
      kind: step.step,
      step,
    }));
    reconcile(
      this.list,
      this.steps,
      entries,
      (entry) => (entry.kind === "tool" ? new ToolStepView() : new ThinkingStepView()),
      (component, entry) => {
        if (component.kind === "tool" && entry.step.step === "tool") {
          component.update(entry.step);
        } else if (component.kind === "thought" && entry.step.step === "thought") {
          component.update(entry.step);
        }
      },
    );
    this.text = row.steps
      .map((step) => (step.step === "tool" ? `${step.label.action} ${step.label.details}`.trim() : step.text))
      .join("\n\n");
  }

  copyText(): string {
    return this.text;
  }

  /**
   * Takes over the operator's choice from the thought row or tool row this
   * group replaced. That row's step is the group's first, under the same
   * key, so its open or closed choice moves to the matching step. A row the
   * operator had open also opens the group, so what they were reading stays
   * on screen instead of folding away inside a closed group.
   */
  carryChoice(kind: "thought" | "tool", key: string, choice: boolean | null): void {
    if (choice === null) {
      return;
    }
    const step = this.steps.get(key);
    if (step === undefined || step.kind !== kind) {
      return;
    }
    step.adoptChoice(choice);
    if (choice) {
      this.collapsible.adoptChoice(true);
    }
  }

  dispose(): void {
    for (const step of this.steps.values()) {
      step.dispose();
    }
    this.steps.clear();
  }
}

// --- Row components -----------------------------------------------------------------

/** Any row component the view reconciles. */
export type RowComponent =
  | HumanMessage
  | MarkdownRowView
  | ThoughtRowView
  | GroupRowView
  | ToolRowView;

/** Builds the component for a row's kind. */
export function createRowComponent(row: TranscriptRow, animateIn: boolean): RowComponent {
  let component: RowComponent;
  switch (row.kind) {
    case "human":
      component = new HumanMessage();
      break;
    case "markdown":
      component = new MarkdownRowView(animateIn);
      break;
    case "thought":
      component = new ThoughtRowView();
      break;
    case "group":
      component = new GroupRowView();
      break;
    case "tool":
      component = new ToolRowView();
      break;
  }
  component.element.classList.add("ws-transcript-row");
  component.element.dataset["rowKey"] = row.key;
  return component;
}

/**
 * Hands what the operator chose from a row component the reconcile just
 * replaced to the one that took its key. Only a thought row or a lone tool
 * row grows into a group under the same key (a group takes its first
 * step's key); the group's matching step inherits the choice.
 */
export function carryRowState(component: RowComponent, replaced: RowComponent, key: string): void {
  if (!(component instanceof GroupRowView)) {
    return;
  }
  if (replaced instanceof ThoughtRowView) {
    component.carryChoice("thought", key, replaced.operatorChoice);
  } else if (replaced instanceof ToolRowView) {
    component.carryChoice("tool", key, replaced.operatorChoice);
  }
}

/** Updates a component with its row; a kind mismatch is a no-op (the reconcile replaces those). */
export function updateRowComponent(component: RowComponent, row: TranscriptRow): void {
  switch (component.kind) {
    case "human":
      if (row.kind === "human") {
        component.update(row);
      }
      return;
    case "markdown":
      if (row.kind === "markdown") {
        component.update(row);
      }
      return;
    case "thought":
      if (row.kind === "thought") {
        component.update(row);
      }
      return;
    case "group":
      if (row.kind === "group") {
        component.update(row);
      }
      return;
    case "tool":
      if (row.kind === "tool") {
        component.update(row);
      }
      return;
  }
}
