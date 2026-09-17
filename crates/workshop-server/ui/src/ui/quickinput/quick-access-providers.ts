// The quick-access providers the quickinput component owns: the ">"
// command palette, the "?" help list, the not-available placeholders
// for "@", "%", "debug ", and "task ", and the descriptor table that
// registers them all in modes-list order. The "" file provider and the
// ":" go-to-line provider belong to the workspace and editor features
// and register from their own contribution files.
//
// The palette reads the CommandPalette menu at every open: a row whose
// command is unregistered or whose precondition fails is absent, labels
// are "Category: Title", each row carries the command's keybinding
// label, and recently used commands (the COMMANDS_HISTORY service) sort
// first in recency order with the rest in menu order. Accepting a
// row records the command in the history and dispatches it; a rejected
// run posts to the status bar, matching the menu widget.
//
// The help provider renders one row per registered provider's help
// entries; accepting a row re-opens quick input at that entry's prefix.
// A placeholder provider renders a single inert "not available" row so
// the modes list matches Cursor's while the mode is unimplemented.
//
// Every factory takes its registries as optional deps defaulting to the
// shared singletons or the service registry; tests inject their own.

import { Commands, type CommandRegistry } from "../../services/command-registry";
import { CONTEXT_KEY_SERVICE, type ContextKeyService } from "../../services/context-key-service";
import { KeybindingsRegistry } from "../../services/keybinding-registry";
import { MenuId, Menus, type MenuRegistry } from "../../services/menu-registry";
import { QuickAccessRegistry, type QuickAccessProviderDescriptor } from "../../services/quick-access-registry";
import { getService, getServiceOrNull } from "../../services/service-registry";
import { STATUS_BAR } from "../status/status-bar";
import { COMMANDS_HISTORY, type CommandsHistory } from "./commands-history";
import { QUICK_INPUT_SERVICE, type QuickAccessProvider, type QuickInputItem } from "./quick-input";

/** The registries the palette provider reads; tests inject their own. */
export interface CommandPaletteProviderDeps {
  readonly commands?: CommandRegistry;
  readonly menus?: MenuRegistry;
  readonly keybindings?: KeybindingsRegistry;
  readonly context?: ContextKeyService;
  readonly history?: CommandsHistory;
}

/** Reports a rejected command run to the status bar, as the menu does. */
function reportCommandFailure(commandId: string, error: unknown): void {
  const statusBar = getServiceOrNull(STATUS_BAR);
  if (statusBar === null) {
    // No composition root (a widget test): keep the failure loud.
    console.error(`palette command '${commandId}' failed`, error);
    return;
  }
  const message = error instanceof Error ? error.message : String(error);
  statusBar.showLocal(`Could not run '${commandId}': ${message}`, "error");
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
            keybinding,
            accept: () => {
              history.add(row.command);
              void commands.execute(row.command, ...args).catch((error: unknown) => {
                reportCommandFailure(row.command, error);
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
      return rows.map((row) => row.item);
    },
  };
}

/** The registry and mode-switch sink the help provider reads. */
export interface HelpProviderDeps {
  readonly registry?: QuickAccessRegistry;
  readonly show?: (value: string) => void;
}

/**
 * The "?" help provider: one row per registered provider's help
 * entries, re-read at every getItems. Accepting a row re-opens quick
 * input at the entry's prefix, entering that mode.
 */
export function createHelpProvider(deps: HelpProviderDeps = {}): QuickAccessProvider {
  const registry = deps.registry ?? QuickAccessRegistry;
  const show =
    deps.show ??
    ((value: string): void => {
      getService(QUICK_INPUT_SERVICE).quickAccess.show(value);
    });
  return {
    getItems(): readonly QuickInputItem[] {
      const rows: QuickInputItem[] = [];
      for (const descriptor of registry.getQuickAccessProviders()) {
        for (const entry of descriptor.helpEntries) {
          rows.push({
            label: entry.description,
            description: entry.prefix === "" ? undefined : entry.prefix,
            accept: () => show(entry.prefix),
          });
        }
      }
      return rows;
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
 * ":" provider from the editor contribution.
 */
export function createQuickAccessProviderDescriptors(
  deps: QuickAccessProviderDeps = {},
): readonly QuickAccessProviderDescriptor[] {
  return [
    {
      prefix: ">",
      placeholder: "Type a command",
      helpEntries: [{ description: "Show and Run Commands", prefix: ">" }],
      factory: () => createCommandPaletteProvider(deps),
    },
    {
      prefix: "%",
      placeholder: "Search for text",
      helpEntries: [{ description: "Search for Text", prefix: "%" }],
      factory: () => createPlaceholderProvider("Text search is not available"),
    },
    {
      prefix: "@",
      placeholder: "Go to symbol in editor",
      helpEntries: [{ description: "Go to Symbol in Editor", prefix: "@" }],
      factory: () => createPlaceholderProvider("Go to symbol in editor is not available"),
    },
    {
      prefix: "debug ",
      placeholder: "Debug configurations",
      helpEntries: [{ description: "Start Debugging", prefix: "debug " }],
      factory: () => createPlaceholderProvider("Debugging is not available"),
    },
    {
      prefix: "task ",
      placeholder: "Run task",
      helpEntries: [{ description: "Run Task", prefix: "task " }],
      factory: () => createPlaceholderProvider("Tasks are not available"),
    },
    {
      prefix: "?",
      placeholder: "Quick access help",
      helpEntries: [{ description: "More", prefix: "?" }],
      factory: () => createHelpProvider({ registry: deps.quickAccess, show: deps.show }),
    },
  ];
}
