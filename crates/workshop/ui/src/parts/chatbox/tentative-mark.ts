// The dictation tentative mark: a ProseMirror decoration plugin, registered
// as a Tiptap extension, that styles each live take's tentative words
// without touching the document. Each take owns at most one range, keyed
// by its take id; a range maps through later edits until the take's next
// range replaces it or a null range clears it.

import { Extension } from "@tiptap/core";
import { Plugin, PluginKey, type Transaction } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";

/** The class the skin paints tentative dictation words with. */
export const TENTATIVE_CLASS = "ws-stt-tentative";

/** One take's tentative range, or null to clear that take's mark. */
interface TentativeMarkUpdate {
  readonly takeId: number;
  readonly range: { readonly from: number; readonly to: number } | null;
}

const tentativeMarkKey = new PluginKey<DecorationSet>("dictationTentativeMark");

/** The extension the chat box registers to show tentative marks. */
export const TentativeMark = Extension.create({
  name: "dictationTentativeMark",
  addProseMirrorPlugins() {
    return [
      new Plugin<DecorationSet>({
        key: tentativeMarkKey,
        state: {
          init: () => DecorationSet.empty,
          apply: (tr, marks) => {
            const mapped = marks.map(tr.mapping, tr.doc);
            // Only setTentativeMark writes this meta, always with this shape.
            const update = tr.getMeta(tentativeMarkKey) as TentativeMarkUpdate | undefined;
            if (update === undefined) {
              return mapped;
            }
            const kept = mapped.remove(
              mapped.find(
                undefined,
                undefined,
                (spec: { readonly takeId?: number }) => spec.takeId === update.takeId,
              ),
            );
            const { range } = update;
            if (
              range === null ||
              range.from < 0 ||
              range.from >= range.to ||
              range.to > tr.doc.content.size
            ) {
              return kept;
            }
            return kept.add(tr.doc, [
              Decoration.inline(
                range.from,
                range.to,
                { class: TENTATIVE_CLASS },
                { takeId: update.takeId },
              ),
            ]);
          },
        },
        props: {
          decorations: (state) => tentativeMarkKey.getState(state),
        },
      }),
    ];
  },
});

/** Sets or clears one take's tentative range on a transaction that changes no text. */
export function setTentativeMark(
  tr: Transaction,
  takeId: number,
  range: TentativeMarkUpdate["range"],
): Transaction {
  const update: TentativeMarkUpdate = { takeId, range };
  return tr.setMeta(tentativeMarkKey, update);
}
