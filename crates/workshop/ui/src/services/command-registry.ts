// The command registry: a Map of command actions keyed by command id
// (the VS Code registerAction2 pattern). Every invocable action - menu
// rows, keybinding rules, command-palette entries - is an action
// registered once and dispatched by id, so the surfaces that trigger a
// command never name the code that performs it. An action is a run
// function plus metadata (title, category, precondition, toggled); the
// presentation extras of the old descriptor (label, shortcut, enabled)
// now live with the surfaces that render them.
//
// The module-level Commands instance is the shared registry the
// composition root and feature contribution files populate; the menu
// widget, the keybinding dispatcher, and quick input read it. Tests
// construct their own instances for isolation.
//
// Generic and DOM-free: nothing here may import from the app layers.

import { toDisposable, type IDisposable } from "../base/lifecycle";

/** One invocable action. */
export interface CommandAction {
  /**
   * Runs the command. Arguments come from the menu row or the caller;
   * the implementation narrows them, never casts.
   */
  readonly run: (...args: readonly unknown[]) => void | Promise<void>;
  /** The display title, e.g. "Save". */
  readonly title?: string;
  /** The category prefix, e.g. "File". */
  readonly category?: string;
  /** The when-expression gating enablement. */
  readonly precondition?: string;
  /** The when-expression driving the checked state. */
  readonly toggled?: string;
}

export class CommandRegistry {
  private readonly commands = new Map<string, CommandAction>();

  /**
   * Registers `action` under `id`. Re-registering an id upserts: the
   * new action replaces the old, so re-running a setup (a test
   * scenario, a hot reload) never duplicates. The returned disposable
   * unregisters only if this registration is still current, so
   * disposing a stale registration cannot evict its replacement.
   */
  register(id: string, action: CommandAction): IDisposable {
    this.commands.set(id, action);
    return toDisposable(() => {
      if (this.commands.get(id) === action) {
        this.commands.delete(id);
      }
    });
  }

  /** The action registered under `id`, if any. */
  lookup(id: string): CommandAction | undefined {
    return this.commands.get(id);
  }

  /**
   * Runs the command registered under `id`, awaiting an async run.
   * Answers false when no command is registered - a shortcut whose
   * owning feature has not activated yet is a no-op, never a crash.
   */
  async execute(id: string, ...args: readonly unknown[]): Promise<boolean> {
    const action = this.commands.get(id);
    if (action === undefined) {
      return false;
    }
    await action.run(...args);
    return true;
  }
}

/** The shared registry the running app dispatches through. */
export const Commands = new CommandRegistry();

/** Registers a command into the shared registry. */
export function registerCommand(id: string, action: CommandAction): IDisposable {
  return Commands.register(id, action);
}

/** Dispatches a command by id through the shared registry. */
export async function executeCommand(id: string, ...args: readonly unknown[]): Promise<boolean> {
  return Commands.execute(id, ...args);
}
