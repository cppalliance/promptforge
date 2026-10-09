// The quick-access providers the quickinput component owns: the ">"
// command palette, the "?" help list, the not-available placeholders
// for "@", "%", "debug ", and "task ", and the descriptor table that
// registers them all in modes-list order. The "" file provider and the
// ":" go-to-line provider belong to the workspace and editor features
// and register from their own contribution files; their wording follows
// the same rule as the table's. Every placeholder and help string is
// Cursor's own, and each help entry carries the command-center order,
// label, and command id Cursor gives it.
//
// The palette reads the CommandPalette menu at every open: a row whose
// command is unregistered or whose precondition fails is absent, labels
// are "Category: Title" with the filter's match highlighted, each row
// shows the command's keybinding label, and recently used commands (the
// COMMANDS_HISTORY service) sort first in recency order with the rest in
// menu order, under the "recently used" and "other commands" group
// labels. Accepting a row records the command in the history and
// dispatches it; a rejected run is reported by the registry (the toast)
// and logged here. With no match the list reads "No matching commands".
//
// The help provider renders one row per registered provider's help
// entries, as Cursor does: the prefix (an ellipsis for the default mode)
// as the label, the help text as the description, sorted by prefix with
// "?" itself left out, and each command's keybinding. Accepting a row
// enters that mode, and typing a mode's prefix after "?" jumps there. A
// placeholder provider renders a single inert "not available" row so the
// modes list matches Cursor's while the mode is unimplemented.
//
// Every factory takes its registries as optional deps defaulting to the
// shared singletons or the service registry; tests inject their own.

import { Commands, logCommandFailure, type CommandRegistry } from "@workshop/platform/command-registry";
import { COMMANDS_HISTORY, type CommandsHistory } from "../../services/commands-history";
import { CONTEXT_KEY_SERVICE, type ContextKeyService } from "@workshop/platform/context-key-service";
import { KeybindingsRegistry } from "@workshop/platform/keybinding-registry";
import { MenuId, Menus, type MenuRegistry } from "@workshop/platform/menu-registry";
import { QuickAccessRegistry, type QuickAccessProviderDescriptor } from "@workshop/platform/quick-access-registry";
import { getService } from "@workshop/platform/service-registry";
import {
  QUICK_INPUT_SERVICE,
  substringHighlights,
  type QuickAccessProvider,
  type QuickInputItem,
} from "../../services/quick-input-service";

/** The prefix that opens the help list. */
const HELP_PREFIX = "?";

/** What the help list labels the default mode, whose prefix is empty. */
const DEFAULT_MODE_LABEL = "\u2026";

/** The registries the palette provider reads; tests inject their own. */
export interface CommandPaletteProviderDeps {
  readonly commands?: CommandRegistry;
  readonly menus?: MenuRegistry;
  readonly keybindings?: KeybindingsRegistry;
  readonly context?: ContextKeyService;
  readonly history?: CommandsHistory;
}

/**
 * The ">" command palette provider. Rows come from the CommandPalette
 * menu, re-read at every getItems so late registrations appear; the
 * filter is a case-insensitive substring match on the rendered label.
 */
export function createCommandPaletteProvider(deps: CommandPaletteProviderDeps = {}): QuickAccessProvider {
  const commands = deps.commands ?? Commands;
  const menus = deps.menus ?? Menus;
  const keybindings = deps.keybindings ?? KeybindingsRegistry;
  const context = deps.context ?? getService(CONTEXT_KEY_SERVICE);
  const history = deps.history ?? getService(COMMANDS_HISTORY);
  return {
    noResultsMessage: "No matching commands",
    getItems(filter: string): readonly QuickInputItem[] {
      const needle = filter.trim().toLowerCase();
      const recent = new Map(history.list.map((id, index) => [id, index]));
      const rows: { readonly commandId: string; readonly item: QuickInputItem }[] = [];
      for (const row of menus.getMenuItems(MenuId.CommandPalette)) {
        if ("submenu" in row) {
          continue;
        }
        const command = commands.lookup(row.command);
        if (command === undefined) {
          continue;
        }
        if (!context.contextMatchesRules(command.precondition)) {
          continue;
        }
        const title = row.title ?? command.title ?? row.command;
        const label = command.category === undefined ? title : `${command.category}: ${title}`;
        if (needle !== "" && !label.toLowerCase().includes(needle)) {
          continue;
        }
        const keybinding = keybindings.lookupKeybinding(row.command)?.getLabel();
        const args = row.args ?? [];
        rows.push({
          commandId: row.command,
          item: {
            label,
            labelHighlights: substringHighlights(label, needle),
            keybinding,
            accept: () => {
              history.add(row.command);
              // The registry reports a failure (the toast); keep the detail.
              void commands.execute(row.command, ...args).catch((error: unknown) => {
                logCommandFailure(row.command, error);
              });
            },
          },
        });
      }
      // Recently used first in recency order; the sort is stable, so the
      // rest keep their menu order.
      rows.sort(
        (a, b) =>
          (recent.get(a.commandId) ?? Number.MAX_SAFE_INTEGER) - (recent.get(b.commandId) ?? Number.MAX_SAFE_INTEGER),
      );
      // The recent run opens under "recently used"; the first command after
      // it opens "other commands".
      return rows.map((row, index) => {
        const isRecent = recent.has(row.commandId);
        if (index === 0 && isRecent) {
          return { ...row.item, separator: "recently used" };
        }
        if (!isRecent && index > 0 && recent.has(rows[index - 1]?.commandId ?? "")) {
          return { ...row.item, separator: "other commands" };
        }
        return row.item;
      });
    },
  };
}

/** The registries and mode-switch sink the help provider reads. */
export interface HelpProviderDeps {
  readonly registry?: QuickAccessRegistry;
  readonly keybindings?: KeybindingsRegistry;
  readonly show?: (value: string) => void;
}

/**
 * The "?" help provider: one row per registered provider's help
 * entries, re-read at every getItems, sorted by the provider's prefix.
 * A row's label is the entry's prefix (the provider's own when the entry
 * has none, an ellipsis for the default mode) and its description is the
 * help text; the "?" entry itself is left out. Accepting a row enters
 * that mode, and the redirect sends "?" followed by another mode's
 * prefix straight into that mode.
 */
export function createHelpProvider(deps: HelpProviderDeps = {}): QuickAccessProvider {
  const registry = deps.registry ?? QuickAccessRegistry;
  const keybindings = deps.keybindings ?? KeybindingsRegistry;
  const show =
    deps.show ??
    ((value: string): void => {
      getService(QUICK_INPUT_SERVICE).quickAccess.show(value);
    });
  return {
    getItems(): readonly QuickInputItem[] {
      const providers = [...registry.getQuickAccessProviders()].sort((a, b) => a.prefix.localeCompare(b.prefix));
      const rows: QuickInputItem[] = [];
      for (const descriptor of providers) {
        for (const entry of descriptor.helpEntries) {
          const prefix = entry.prefix || descriptor.prefix;
          if (prefix === HELP_PREFIX) {
            continue;
          }
          rows.push({
            label: prefix === "" ? DEFAULT_MODE_LABEL : prefix,
            description: entry.description,
            keybinding:
              entry.commandId === undefined ? undefined : keybindings.lookupKeybinding(entry.commandId)?.getLabel(),
            accept: () => show(prefix),
          });
        }
      }
      return rows;
    },
    redirect(filter: string): string | undefined {
      const target = registry.getQuickAccessProvider(filter);
      return target !== undefined && target.prefix !== "" && target.prefix !== HELP_PREFIX ? target.prefix : undefined;
    },
  };
}

/**
 * A placeholder provider for a mode Cursor lists but this build does
 * not implement: one inert row telling the truth about availability.
 */
export function createPlaceholderProvider(message: string): QuickAccessProvider {
  return {
    getItems(): readonly QuickInputItem[] {
      return [{ label: message, accept: () => {} }];
    },
  };
}

/** The deps the descriptor table threads into the palette and help providers. */
export interface QuickAccessProviderDeps extends CommandPaletteProviderDeps {
  /** The registry the help provider reads; defaults to the shared one. */
  readonly quickAccess?: QuickAccessRegistry;
  /** The mode-switch sink the help provider's rows accept into. */
  readonly show?: (value: string) => void;
}

/**
 * The quickinput-owned provider descriptors in modes-list order: ">"
 * Show and Run Commands, "%" Search for Text, "@" Go to Symbol in
 * Editor, "debug " Start Debugging, "task " Run Task, "?" More. The ""
 * Go to File provider registers from the workspace contribution and the
 * ":" provider from the editor contribution. Every string is Cursor's;
 * the command ids are the ones Cursor's help entries name, so the lists
 * show a keybinding for whichever of them this build binds.
 */
export function createQuickAccessProviderDescriptors(
  deps: QuickAccessProviderDeps = {},
): readonly QuickAccessProviderDescriptor[] {
  return [
    {
      prefix: ">",
      placeholder: "Type the name of a command to run.",
      helpEntries: [
        {
          description: "Show and Run Commands",
          prefix: ">",
          commandId: "workbench.action.showCommands",
          commandCenterOrder: 20,
        },
      ],
      factory: () => createCommandPaletteProvider(deps),
    },
    {
      prefix: "%",
      placeholder: "Search for text in your workspace files.",
      helpEntries: [
        {
          description: "Search for Text",
          prefix: "%",
          commandId: "workbench.action.quickTextSearch",
          commandCenterOrder: 25,
        },
      ],
      factory: () => createPlaceholderProvider("Text search is not available"),
    },
    {
      prefix: "@",
      placeholder: "Type the name of a symbol to go to.",
      helpEntries: [
        {
          description: "Go to Symbol in Editor",
          prefix: "@",
          commandId: "workbench.action.gotoSymbol",
          commandCenterOrder: 40,
        },
        { description: "Go to Symbol in Editor by Category", prefix: "@:" },
      ],
      factory: () => createPlaceholderProvider("Go to symbol in editor is not available"),
    },
    {
      prefix: "debug ",
      placeholder: "Type the name of a launch configuration to run.",
      helpEntries: [
        {
          description: "Start Debugging",
          prefix: "debug ",
          commandId: "workbench.action.debug.selectandstart",
          commandCenterOrder: 50,
        },
      ],
      factory: () => createPlaceholderProvider("Debugging is not available"),
    },
    {
      prefix: "task ",
      placeholder: "Type the name of a task to run.",
      helpEntries: [{ description: "Run Task", prefix: "task ", commandCenterOrder: 60 }],
      factory: () => createPlaceholderProvider("Tasks are not available"),
    },
    {
      prefix: HELP_PREFIX,
      placeholder: `Type '${HELP_PREFIX}' to get help on the actions you can take from here.`,
      helpEntries: [
        { description: "Show all Quick Access Providers", commandCenterOrder: 70, commandCenterLabel: "More" },
      ],
      factory: () =>
        createHelpProvider({ registry: deps.quickAccess, keybindings: deps.keybindings, show: deps.show }),
    },
  ];
}
