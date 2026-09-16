// The action registry: the single registration point every feature
// contribution file calls (the VS Code registerAction2 pattern). One
// action descriptor fans out into the three registries from steps 1-3:
// the command registry gets the command with its metadata, the menu
// registry gets one row per menu entry plus a CommandPalette row when
// f1 is set, and the keybinding registry gets the keybinding rule with
// the action's precondition ANDed into the rule's when, so a chord
// never fires while the command's precondition fails.
//
// Every when, precondition, toggled, and keybinding string is parsed
// once at registration; a malformed string comes back as a ParseError
// value with nothing registered, so a bad contribution can never throw
// at render or dispatch. A successful registration returns one
// DisposableStore that unwinds the command, the menu rows, and the
// keybinding rule together.
//
// The module-level registerAction writes to the shared Commands, Menus,
// and KeybindingsRegistry singletons; tests build their own with
// createActionRegistry.
//
// Generic and DOM-free: nothing here may import from the app layers.

import { DisposableStore, type IDisposable } from "../base/lifecycle";
import { Commands, type CommandRegistry } from "./command-registry";
import { ContextKeyExpr, type ParseError } from "./context-key-expr";
import { err, ok, type Result } from "./error-catalog";
import { detectPlatform, parseKeybinding, type KeybindingPlatform } from "./keybinding-parser";
import { KeybindingsRegistry, type KeybindingWeight } from "./keybinding-registry";
import { MenuId, Menus, type MenuRegistry } from "./menu-registry";

/** One menu placement for an action. */
export interface ActionMenuEntry {
  /** The menu the row lands in. */
  readonly id: MenuId;
  /** The sort group; "navigation" sorts first. */
  readonly group?: string;
  /** The position within the group. */
  readonly order?: number;
  /** The when-expression gating the row's visibility. */
  readonly when?: string;
  /** The arguments passed to the command's run. */
  readonly args?: readonly unknown[];
}

/** The action's keybinding, in the shape of VS Code's keybinding contribution. */
export interface ActionKeybinding {
  /** The chord string, e.g. "ctrlcmd+s" or "ctrl+m ctrl+o". */
  readonly keybinding: string;
  /** The when-expression gating the rule; the precondition is ANDed in. */
  readonly when?: string;
  /** The weight tier; defaults to WorkbenchContrib. */
  readonly weight?: KeybindingWeight;
}

/** One action: a command plus its menu, palette, and keybinding placements. */
export interface ActionDescriptor {
  /** The command id, e.g. "workbench.action.files.save". */
  readonly id: string;
  /** The display title, e.g. "Save". */
  readonly title: string;
  /** The category prefix, e.g. "File". */
  readonly category?: string;
  /** When true, the action appears in the command palette. */
  readonly f1?: boolean;
  /** The when-expression gating enablement; ANDed into the keybinding's when. */
  readonly precondition?: string;
  /** The when-expression driving the checked state. */
  readonly toggled?: string;
  /** The action's keybinding rule. */
  readonly keybinding?: ActionKeybinding;
  /** The menus the action appears in. */
  readonly menu?: readonly ActionMenuEntry[];
  /** Runs the command; arguments come from the menu row or the caller. */
  readonly run: (...args: readonly unknown[]) => void | Promise<void>;
}

/** The registries an action registry writes to, plus the chord platform. */
export interface ActionRegistryDeps {
  readonly commands: CommandRegistry;
  readonly menus: MenuRegistry;
  readonly keybindings: KeybindingsRegistry;
  /** The platform chord strings parse against (the ctrlcmd resolution). */
  readonly platform: KeybindingPlatform;
}

/** The action registrar over one set of registries. */
export interface ActionRegistry {
  /**
   * Registers one action across the command, menu, and keybinding
   * registries. Every expression and chord string is parsed first; a
   * malformed string returns a ParseError and registers nothing. On
   * success the returned DisposableStore unwinds the command, every
   * menu row, and the keybinding rule together.
   */
  registerAction(action: ActionDescriptor): Result<IDisposable, ParseError>;
}

/** Parses a when-expression, tagging the failure with the field name. */
function validateWhen(field: string, when: string | undefined): Result<undefined, ParseError> {
  if (when === undefined) {
    return ok(undefined);
  }
  const parsed = ContextKeyExpr.deserialize(when);
  if (!parsed.ok) {
    return err({ message: `${field} '${when}' is malformed: ${parsed.error.message}`, offset: parsed.error.offset });
  }
  return ok(undefined);
}

/** Joins the precondition and a rule when into one ANDed expression. */
function combineWhen(precondition: string | undefined, when: string | undefined): string | undefined {
  if (precondition !== undefined && when !== undefined) {
    return `(${precondition}) && (${when})`;
  }
  return precondition ?? when;
}

/** Builds an action registry over `deps`. */
export function createActionRegistry(deps: ActionRegistryDeps): ActionRegistry {
  function registerAction(action: ActionDescriptor): Result<IDisposable, ParseError> {
    const precondition = validateWhen("precondition", action.precondition);
    if (!precondition.ok) {
      return precondition;
    }
    const toggled = validateWhen("toggled", action.toggled);
    if (!toggled.ok) {
      return toggled;
    }
    for (const entry of action.menu ?? []) {
      const menuWhen = validateWhen(`menu '${entry.id}' when`, entry.when);
      if (!menuWhen.ok) {
        return menuWhen;
      }
    }
    if (action.keybinding !== undefined) {
      const chord = parseKeybinding(action.keybinding.keybinding, deps.platform);
      if (!chord.ok) {
        return err({
          message: `keybinding '${action.keybinding.keybinding}' for '${action.id}' is malformed: ${chord.error.message}`,
          offset: chord.error.offset,
        });
      }
      const keyWhen = validateWhen("keybinding when", action.keybinding.when);
      if (!keyWhen.ok) {
        return keyWhen;
      }
    }

    const store = new DisposableStore();
    store.add(
      deps.commands.register(action.id, {
        run: action.run,
        title: action.title,
        category: action.category,
        precondition: action.precondition,
        toggled: action.toggled,
      }),
    );
    for (const entry of action.menu ?? []) {
      store.add(
        deps.menus.appendMenuItem(entry.id, {
          command: action.id,
          args: entry.args,
          when: entry.when,
          group: entry.group,
          order: entry.order,
        }),
      );
    }
    if (action.f1 === true) {
      store.add(deps.menus.appendMenuItem(MenuId.CommandPalette, { command: action.id }));
    }
    if (action.keybinding !== undefined) {
      store.add(
        deps.keybindings.registerKeybindingRule({
          id: action.id,
          keybinding: action.keybinding.keybinding,
          when: combineWhen(action.precondition, action.keybinding.when),
          weight: action.keybinding.weight,
        }),
      );
    }
    return ok(store);
  }

  return { registerAction };
}

const shared = createActionRegistry({
  commands: Commands,
  menus: Menus,
  keybindings: KeybindingsRegistry,
  platform: detectPlatform(),
});

/** Registers one action into the shared registries. */
export function registerAction(action: ActionDescriptor): Result<IDisposable, ParseError> {
  return shared.registerAction(action);
}
