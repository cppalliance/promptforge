// Cursor's toast for a failed command: "Command '{0}' resulted in an error",
// with {0} the command's palette label (Category: Title, as the palette row
// reads, so the toast names what the operator picked). The platform registry
// reports each failure (onDidFailCommand) because @workshop/platform never
// imports the app; this module is the app's half, turning the report into an
// error toast on the shared stack. The toast carries no error text, as
// Cursor's carries none; the callers that dispatch a command log the error
// object on the console, so the detail is not lost.

import type { ToastStack } from "@workshop/look/toast";
import { Commands, logCommandFailure, type CommandRegistry } from "@workshop/platform/command-registry";
import type { IDisposable } from "@workshop/platform/lifecycle";
import { getServiceOrNull } from "@workshop/platform/service-registry";
import { TOAST_STACK } from "./toast-service";

/** The toast text for a failed command whose palette label is `label`. */
function failureMessage(label: string): string {
  return `Command '${label}' resulted in an error`;
}

/** Raises an error toast for every command `commands` reports as failed. */
export function toastCommandFailures(commands: CommandRegistry, toasts: ToastStack): IDisposable {
  return commands.onDidFailCommand(({ label }) => {
    toasts.show(failureMessage(label), "error");
  });
}

/**
 * Reports a failure of a command run that bypassed the registry (a direct
 * call into the layout core), which the registry's report cannot see: the
 * same toast on the shared stack, when one is registered, and the error
 * object on the console. A bare widget with no stack leaves the console.
 */
export function toastCommandFailure(commandId: string, error: unknown, commands: CommandRegistry = Commands): void {
  logCommandFailure(commandId, error);
  getServiceOrNull(TOAST_STACK)?.show(failureMessage(commands.labelOf(commandId)), "error");
}
