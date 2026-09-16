// The keybindings registry: the module-level store of keybinding rules
// (the shape of VS Code's IKeybindingRule) that feature contribution
// files write at module scope and the dispatcher reads through a
// resolver snapshot. Each rule names a command id and a chord string,
// with optional mac and linux overrides so one row binds Cmd+S on macOS
// and Ctrl+S elsewhere; the ctrlcmd token inside the chord string covers
// the common case without an override.
//
// Chord and when strings are parsed once at registration; a malformed
// string is reported to the console once and the rule is dropped, so a
// bad contribution can never throw at dispatch. Registration returns a
// disposable that removes only its own rule.
//
// The module-level KeybindingsRegistry singleton resolves against the
// detected platform; tests build their own with createKeybindingsRegistry.
//
// Generic and DOM-free: nothing here may import from the app layers.

import { toDisposable, type IDisposable } from "../base/lifecycle";
import { ContextKeyExpr, type ContextKeyExpression } from "./context-key-expr";
import { detectPlatform, formatKeybinding, parseKeybinding, type KeybindingPlatform } from "./keybinding-parser";
import { KeybindingResolver, type ResolvedKeybindingRule } from "./keybinding-resolver";

/**
 * VS Code's weight tiers. Editor actions register at EditorContrib,
 * workbench actions at WorkbenchContrib (the default); higher tiers win
 * when several rules claim the same chord.
 */
export const KeybindingWeight = {
  EditorCore: 0,
  EditorContrib: 100,
  WorkbenchContrib: 200,
  BuiltinExtension: 300,
  ExternalExtension: 400,
} as const;

/** A weight tier from the KeybindingWeight const object. */
export type KeybindingWeight = (typeof KeybindingWeight)[keyof typeof KeybindingWeight];

/** One keybinding rule, in the shape of VS Code's IKeybindingRule. */
export interface KeybindingRule {
  /** The command the chord dispatches. */
  readonly id: string;
  /** The chord string, e.g. "ctrlcmd+s" or "ctrl+m ctrl+o". */
  readonly keybinding: string;
  /** The macOS chord, when it differs from the default. */
  readonly mac?: string;
  /** The Linux chord, when it differs from the default. */
  readonly linux?: string;
  /** The when-expression gating the rule. */
  readonly when?: string;
  /** The weight tier; defaults to WorkbenchContrib. */
  readonly weight?: KeybindingWeight;
}

/** The label for a command's keybinding, rendered per platform. */
export interface KeybindingLabel {
  getLabel(): string;
}

/** The rule store the dispatcher and menu labels read. */
export interface KeybindingsRegistry {
  /**
   * Registers one rule. The chord string (or the running platform's
   * override) and the when string are parsed now; a malformed string is
   * reported once and the rule is dropped. The returned disposable
   * removes only this registration.
   */
  registerKeybindingRule(rule: KeybindingRule): IDisposable;
  /** A resolver snapshot over the currently registered rules. */
  getResolver(): KeybindingResolver;
  /**
   * The display label for a command's keybinding, or undefined when no
   * rule binds it. With several rules for one command, the first
   * registered is the label.
   */
  lookupKeybinding(commandId: string): KeybindingLabel | undefined;
}

interface StoredRule {
  readonly resolved: ResolvedKeybindingRule;
  readonly chords: ResolvedKeybindingRule["chords"];
}

/** Builds a registry resolving chord strings against `platform`. */
export function createKeybindingsRegistry(platform: KeybindingPlatform): KeybindingsRegistry {
  const rules: StoredRule[] = [];
  let nextOrder = 0;

  function registerKeybindingRule(rule: KeybindingRule): IDisposable {
    const source = platform === "mac" ? (rule.mac ?? rule.keybinding) : platform === "linux" ? (rule.linux ?? rule.keybinding) : rule.keybinding;
    const parsed = parseKeybinding(source, platform);
    if (!parsed.ok) {
      console.error(`keybinding for '${rule.id}' is malformed: ${parsed.error.message} at offset ${parsed.error.offset}`);
      return toDisposable(() => {});
    }
    let when: ContextKeyExpression | undefined;
    if (rule.when !== undefined) {
      const parsedWhen = ContextKeyExpr.deserialize(rule.when);
      if (!parsedWhen.ok) {
        console.error(`when for keybinding '${rule.id}' is malformed: ${parsedWhen.error.message} at offset ${parsedWhen.error.offset}`);
        return toDisposable(() => {});
      }
      when = parsedWhen.value;
    }
    const stored: StoredRule = {
      chords: parsed.value,
      resolved: {
        commandId: rule.id,
        chords: parsed.value,
        when,
        weight: rule.weight ?? KeybindingWeight.WorkbenchContrib,
        order: nextOrder,
      },
    };
    nextOrder += 1;
    rules.push(stored);
    return toDisposable(() => {
      const index = rules.indexOf(stored);
      if (index !== -1) {
        rules.splice(index, 1);
      }
    });
  }

  return {
    registerKeybindingRule,
    getResolver(): KeybindingResolver {
      return new KeybindingResolver(rules.map((rule) => rule.resolved));
    },
    lookupKeybinding(commandId: string): KeybindingLabel | undefined {
      const found = rules.find((rule) => rule.resolved.commandId === commandId);
      if (found === undefined) {
        return undefined;
      }
      return { getLabel: () => formatKeybinding(found.chords, platform) };
    },
  };
}

/** The shared registry the running app's contributions populate. */
export const KeybindingsRegistry: KeybindingsRegistry = createKeybindingsRegistry(detectPlatform());
