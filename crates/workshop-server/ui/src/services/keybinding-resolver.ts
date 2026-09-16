// The keybinding resolver: a pure, DOM-free lookup from pressed chords
// to a command id. The dispatcher feeds it the chords pressed so far;
// it answers KbFound when a rule's full sequence matched (and its `when`
// passes against the context service), MoreChordsNeeded when the pressed
// chords are a strict prefix of some rule, and NoMatchingKb otherwise.
// Among exact matches the highest weight wins; at equal weight the last
// registered rule wins, so a feature can override a built-in binding.
//
// hasRuleForChord answers whether any rule starts with the chord
// regardless of `when`: the dispatcher uses it to decide whether to
// swallow a key whose rule is context-gated, so a claimed chord never
// falls through to CodeMirror or the webview default.
//
// Generic and DOM-free: nothing here may import from the app layers.

import { chordsEqual, type Chord } from "./keybinding-parser";
import type { ContextKeyExpression } from "./context-key-expr";
import type { ContextKeyService } from "./context-key-service";

/**
 * One registered rule in resolver form: the command to run, the chord
 * sequence already resolved for the running platform, the parsed (or
 * source) when expression, the weight tier, and the registration order
 * used for the equal-weight tiebreak.
 */
export interface ResolvedKeybindingRule {
  readonly commandId: string;
  readonly chords: readonly Chord[];
  readonly when?: string | ContextKeyExpression;
  readonly weight: number;
  readonly order: number;
}

/** The resolver's answer for one pressed-chord prefix. */
export type KeybindingResolveResult =
  | { readonly kind: "NoMatchingKb" }
  | { readonly kind: "MoreChordsNeeded" }
  | { readonly kind: "KbFound"; readonly commandId: string };

function isPrefix(pressed: readonly Chord[], chords: readonly Chord[]): boolean {
  if (pressed.length > chords.length) {
    return false;
  }
  return pressed.every((chord, index) => chordsEqual(chord, chords[index] as Chord));
}

/** Pure chord-sequence resolution over a snapshot of rules. */
export class KeybindingResolver {
  constructor(private readonly rules: readonly ResolvedKeybindingRule[]) {}

  /**
   * Resolves the chords pressed so far against the context. Exact
   * matches whose when passes win by weight, then by registration order;
   * when no exact match passes but a longer rule shares the prefix, the
   * answer is MoreChordsNeeded.
   */
  resolve(context: Pick<ContextKeyService, "contextMatchesRules">, pressedChords: readonly Chord[]): KeybindingResolveResult {
    const candidates = this.rules.filter((rule) => isPrefix(pressedChords, rule.chords));
    let best: ResolvedKeybindingRule | undefined;
    for (const rule of candidates) {
      if (rule.chords.length !== pressedChords.length || !context.contextMatchesRules(rule.when)) {
        continue;
      }
      if (best === undefined || rule.weight > best.weight || (rule.weight === best.weight && rule.order > best.order)) {
        best = rule;
      }
    }
    if (best !== undefined) {
      return { kind: "KbFound", commandId: best.commandId };
    }
    if (candidates.some((rule) => rule.chords.length > pressedChords.length)) {
      return { kind: "MoreChordsNeeded" };
    }
    return { kind: "NoMatchingKb" };
  }

  /**
   * Answers whether any rule's sequence starts with `chord`, regardless
   * of when - the dispatcher swallows claimed chords even when their
   * rule is context-gated.
   */
  hasRuleForChord(chord: Chord): boolean {
    return this.rules.some((rule) => isPrefix([chord], rule.chords));
  }
}
