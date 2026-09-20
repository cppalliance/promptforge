// The static draft renderer: a SerializedDraft drawn as read-only DOM
// with no editor behind it. The feed shows a sent turn through this, so
// what the operator sees after sending matches what the box showed
// before - the same pill function, the same paragraph and hard-break
// structure - at the cost of one DocumentFragment instead of a
// ProseMirror view. Only the plain-text schema the box produces is
// rendered: paragraphs, text, hard breaks, and mention chips; anything
// else in the document is skipped rather than guessed at.

import type { JSONContent } from "@tiptap/core";
import { renderChip } from "./chip-view";
import { type ChipNodeAttrs, chipFromAttrs } from "./mention-chip";
import type { ChipRef, SerializedDraft } from "./types";

/** A pill for read-only display: no remove button, there is nothing to remove. */
function renderStaticChip(chip: ChipRef): HTMLElement {
  return renderChip(chip, { removable: false });
}

/** Appends one inline node's rendering to `paragraph`; unknown node types render nothing. */
function renderInline(paragraph: HTMLElement, node: JSONContent): void {
  switch (node.type) {
    case "text":
      paragraph.appendChild(document.createTextNode(node.text ?? ""));
      return;
    case "hardBreak":
      paragraph.appendChild(document.createElement("br"));
      return;
    case "mentionNode":
      paragraph.appendChild(renderStaticChip(chipFromAttrs((node.attrs ?? {}) as ChipNodeAttrs)));
      return;
    default:
      return;
  }
}

/**
 * Renders a draft as `ws-draft-view`: the attachments strip
 * (`ws-draft-view__strip`, one pill per attachment, present even when
 * empty so the skin can collapse it), then one `ws-draft-view__paragraph`
 * per paragraph node with its text, hard breaks, and inline chips in
 * document order. The fragment holds exactly that one root element.
 */
export function renderDraft(draft: SerializedDraft): DocumentFragment {
  const fragment = document.createDocumentFragment();
  const root = document.createElement("div");
  root.className = "ws-draft-view";

  const strip = document.createElement("div");
  strip.className = "ws-draft-view__strip";
  for (const attachment of draft.attachments) {
    strip.appendChild(renderStaticChip(attachment));
  }
  root.appendChild(strip);

  for (const block of draft.doc.content ?? []) {
    if (block.type !== "paragraph") {
      continue;
    }
    const paragraph = document.createElement("p");
    paragraph.className = "ws-draft-view__paragraph";
    for (const inline of block.content ?? []) {
      renderInline(paragraph, inline);
    }
    root.appendChild(paragraph);
  }

  fragment.appendChild(root);
  return fragment;
}
