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

import { Mention } from "@tiptap/extension-mention";
import type { MentionNodeAttrs } from "@tiptap/extension-mention";
import { PluginKey } from "@tiptap/pm/state";
import { File, X, createElement } from "lucide";
import { mentionTypeaheadItems, renderMentionTypeahead } from "./typeahead-popup";

const ICON_SIZE_PX = 12;

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

  addNodeView() {
    return ({ node, editor, getPos, HTMLAttributes }) => {
      // The library types attrs as an open record; the extension's own
      // attribute definitions (id, label, mentionSuggestionChar) are the
      // only writers, so the cast narrows to what the schema holds.
      const attrs = node.attrs as MentionNodeAttrs;

      const dom = document.createElement("span");
      dom.className = "mention-chip";
      // setAttribute, not the contentEditable property: jsdom does not
      // reflect the property onto the attribute.
      dom.setAttribute("contenteditable", "false");
      for (const [name, value] of Object.entries(HTMLAttributes)) {
        // The chip owns its class; the remaining rendered attributes
        // (data-id, data-label, data-mention-suggestion-char) carry over.
        if (name === "class") {
          continue;
        }
        dom.setAttribute(name, String(value));
      }

      const icon = document.createElement("span");
      icon.className = "mention-chip__icon";
      icon.setAttribute("aria-hidden", "true");
      icon.appendChild(createElement(File, { width: ICON_SIZE_PX, height: ICON_SIZE_PX }));

      const label = document.createElement("span");
      label.className = "mention-chip__label";
      label.textContent = attrs.label ?? attrs.id ?? "";

      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "mention-chip__remove";
      remove.setAttribute("aria-label", "Remove");
      remove.appendChild(createElement(X, { width: ICON_SIZE_PX, height: ICON_SIZE_PX }));
      remove.addEventListener("click", () => {
        const pos = getPos();
        if (pos === undefined) {
          return;
        }
        editor.chain().deleteRange({ from: pos, to: pos + node.nodeSize }).run();
      });

      dom.append(icon, label, remove);

      return {
        dom,
        // Pointer activity on the remove button belongs to the chip:
        // without this ProseMirror reads the mousedown as the start of a
        // selection or drag on the atom node.
        stopEvent(event) {
          const target = event.target as HTMLElement | null;
          return target !== null && remove.contains(target);
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
