// The mention chip: an inline pill for an @-referenced workspace file,
// rendered inside the prompt editor. Built by extending the official
// Mention extension - the schema, attributes, parse rules, and suggestion
// command stay upstream's; only the NodeView (the live DOM) is ours.
// extend({ name: "mentionNode" }) renames the registered node type to
// match Cursor's ProseMirror JSON schema, so serialized docs compare
// cleanly against Cursor's; the suggestion command, parse rule, and
// Backspace shortcut all read this.name, so they follow the rename
// automatically. (The rename must happen in extend: configure() merges
// its argument into the options and explicitly keeps the parent name.)
// The node carries the chip model beyond upstream's id and label: kind,
// icon, preview, tone, and the opaque host payload `data`, each written
// to and read from a data attribute so the pill survives the clipboard
// (copy renders HTML, paste parses it) and JSON persistence alike.

import { Mention } from "@tiptap/extension-mention";
import type { MentionNodeAttrs } from "@tiptap/extension-mention";
import { PluginKey } from "@tiptap/pm/state";
import { renderChip } from "./chip-view";
import type { ChipRef, JsonValue } from "./types";
import { mentionTypeaheadItems, renderMentionTypeahead } from "./typeahead-popup";

/** The node's attribute set: upstream's three plus the chip model. Absent fields are null. */
export interface ChipNodeAttrs extends MentionNodeAttrs {
  readonly kind?: string | null;
  readonly icon?: string | null;
  readonly preview?: string | null;
  readonly tone?: ChipRef["tone"] | null;
  readonly data?: JsonValue | null;
}

const TONES: ReadonlySet<string> = new Set(["default", "expired", "uploading"]);

/** A tone attribute, or null for anything outside the model's vocabulary. */
function parseTone(value: string | null): ChipRef["tone"] | null {
  return value !== null && TONES.has(value) ? (value as ChipRef["tone"]) : null;
}

/**
 * The JSON payload attribute, or null when absent or unparseable: a
 * pasted pill with a mangled payload becomes a chip without one rather
 * than a paste that throws.
 */
function parsePayload(value: string | null): JsonValue | null {
  if (value === null) {
    return null;
  }
  try {
    return JSON.parse(value) as JsonValue;
  } catch {
    return null;
  }
}

/**
 * The chip a node's attributes describe. The label falls back to the id,
 * and null attributes read as absent fields.
 */
export function chipFromAttrs(attrs: ChipNodeAttrs): ChipRef {
  const chip: {
    id: string;
    label: string;
    kind?: string;
    icon?: string;
    preview?: string;
    tone?: ChipRef["tone"];
    data: JsonValue;
  } = {
    id: attrs.id ?? "",
    label: attrs.label ?? attrs.id ?? "",
    data: attrs.data ?? null,
  };
  if (attrs.kind != null) {
    chip.kind = attrs.kind;
  }
  if (attrs.icon != null) {
    chip.icon = attrs.icon;
  }
  if (attrs.preview != null) {
    chip.preview = attrs.preview;
  }
  if (attrs.tone != null) {
    chip.tone = attrs.tone;
  }
  return chip;
}

/** One optional string attribute mirrored onto `data-<name>`, absent when null. */
function stringAttribute(name: string) {
  return {
    default: null,
    parseHTML: (element: HTMLElement) => element.getAttribute(`data-${name}`),
    renderHTML: (attributes: Record<string, unknown>) => {
      const value = attributes[name];
      return typeof value === "string" ? { [`data-${name}`]: value } : {};
    },
  };
}

/** The slice of the suggestion session state read outside the popup. */
interface MentionSuggestionState {
  readonly active: boolean;
}

/**
 * The plugin key of the mention suggestion session. The prompt input's
 * Enter handling reads it to yield while the typeahead is open:
 * editorProps handlers run before state plugins, so without the state
 * check a submitting Enter would fire instead of the typeahead's
 * selection.
 */
export const MentionSuggestionPluginKey = new PluginKey<MentionSuggestionState>(
  "mentionNodeSuggestion",
);

/**
 * The configured mention extension: upstream Mention renamed to
 * `mentionNode`, with a vanilla-DOM NodeView rendering the pill (icon
 * slot, truncated label, remove button). Registered in PromptInput's
 * extensions array.
 */
export const MentionChip = Mention.extend({
  name: "mentionNode",

  addAttributes() {
    return {
      ...this.parent?.(),
      kind: stringAttribute("kind"),
      icon: stringAttribute("icon"),
      preview: stringAttribute("preview"),
      tone: {
        default: null,
        parseHTML: (element: HTMLElement) => parseTone(element.getAttribute("data-tone")),
        renderHTML: (attributes: Record<string, unknown>) => {
          const value = attributes["tone"];
          return typeof value === "string" && TONES.has(value) ? { "data-tone": value } : {};
        },
      },
      // The opaque host payload travels as one JSON-encoded attribute,
      // so whatever the host put in comes back byte-for-byte.
      data: {
        default: null,
        parseHTML: (element: HTMLElement) => parsePayload(element.getAttribute("data-payload")),
        renderHTML: (attributes: Record<string, unknown>) => {
          const value = attributes["data"];
          return value === null || value === undefined
            ? {}
            : { "data-payload": JSON.stringify(value) };
        },
      },
    };
  },

  addNodeView() {
    return ({ node, editor, getPos, HTMLAttributes }) => {
      // The library types attrs as an open record; the extension's own
      // attribute definitions are the only writers, so the cast narrows
      // to what the schema holds.
      const dom = renderChip(chipFromAttrs(node.attrs as ChipNodeAttrs));
      for (const [name, value] of Object.entries(HTMLAttributes)) {
        // The chip owns its class; the remaining rendered attributes
        // (data-id, data-label, data-mention-suggestion-char, and the
        // chip model's data-*) carry over.
        if (name === "class") {
          continue;
        }
        dom.setAttribute(name, String(value));
      }

      const remove = dom.querySelector(".ws-mention-chip__remove");
      remove?.addEventListener("click", () => {
        const pos = getPos();
        if (pos === undefined) {
          return;
        }
        editor.chain().deleteRange({ from: pos, to: pos + node.nodeSize }).run();
      });

      return {
        dom,
        // Pointer activity on the remove button belongs to the chip:
        // without this ProseMirror reads the mousedown as the start of a
        // selection or drag on the atom node.
        stopEvent(event) {
          const target = event.target as HTMLElement | null;
          return target !== null && remove !== null && remove.contains(target);
        },
      };
    };
  },
}).configure({
  suggestion: {
    char: "@",
    // A named key instead of the extension's anonymous default, so the
    // prompt input can read the session state through it.
    pluginKey: MentionSuggestionPluginKey,
    items: ({ query }) => mentionTypeaheadItems(query),
    render: renderMentionTypeahead,
  },
});
