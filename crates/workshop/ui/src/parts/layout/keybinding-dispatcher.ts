// The keybinding dispatcher: the single document-level listener that
// turns keydown events into command dispatches. It listens in the
// capture phase because CodeMirror's keymap runs on its content element
// and a bubbling listener would fire after it (Alt+Up would move the
// line twice); on a handled result it preventDefaults and
// stopPropagations, so a chord claimed by any rule is swallowed even
// when its `when` fails - Ctrl+F with no editor focused never reaches
// CodeMirror's panel and Ctrl+S never reaches the webview default.
// Unbound keys fall through untouched, so CodeMirror's defaultKeymap
// and searchKeymap keep every binding the registry does not claim.
//
// Multi-key chords: a press that is a strict prefix of some rule sets
// the chordPending context key, posts the waiting status, and arms a
// five-second timer; the next press either completes the chord (the
// command runs), mismatches (the not-a-command status shows for three
// seconds), or never comes (the timer or a window blur abandons the
// chord). A rejected command is caught and reported to the status bar,
// never thrown out of the listener.
//
// The composition root constructs one dispatcher at boot; tests inject
// their own registries and a recording status sink.

import { Disposable, toDisposable } from "../../base/lifecycle";
import { Commands, type CommandRegistry } from "../../services/command-registry";
import { CONTEXT_KEY_SERVICE, type ContextKey, type ContextKeyService } from "../../services/context-key-service";
import { chordFromKeyboardEvent, formatChord, formatKeybinding, type Chord } from "../../services/keybinding-parser";
import { KeybindingsRegistry } from "../../services/keybinding-registry";
import { getService, getServiceOrNull } from "../../services/service-registry";
import { STATUS_BAR } from "../status/status-bar";

/** How long a chord prefix waits for its second key. */
const CHORD_TIMEOUT_MS = 5000;

/** How long the not-a-command status stays on the bar. */
const TRANSIENT_STATUS_MS = 3000;

/**
 * Where the dispatcher's transient messages go. The default sink paints
 * onto the status bar; tests inject a recording sink.
 */
export interface KeybindingStatusSink {
  /** Shows an informational message (the chord prompts). */
  show(message: string): void;
  /** Shows an error message (a command's rejection). */
  showError(message: string): void;
  /** Clears a message whose lifetime ended. */
  clear(): void;
}

/** Registry and service overrides; tests inject their own instances. */
export interface KeybindingDispatcherDependencies {
  readonly commands?: CommandRegistry;
  readonly contextKeys?: ContextKeyService;
  readonly keybindings?: KeybindingsRegistry;
  readonly status?: KeybindingStatusSink;
}

/**
 * The status-bar-backed sink. Local messages go through showLocal, which the
 * next observer frame overwrites; clear restores the idle text. With no
 * composition root (a widget test) the messages go to the console so a
 * swallowed failure stays loud.
 */
function createStatusBarSink(): KeybindingStatusSink {
  return {
    show(message: string): void {
      const statusBar = getServiceOrNull(STATUS_BAR);
      if (statusBar === null) {
        console.info(message);
        return;
      }
      statusBar.showLocal(message, "info");
    },
    showError(message: string): void {
      const statusBar = getServiceOrNull(STATUS_BAR);
      if (statusBar === null) {
        console.error(message);
        return;
      }
      statusBar.showLocal(message, "error");
    },
    clear(): void {
      getServiceOrNull(STATUS_BAR)?.showLocal("Ready", "info");
    },
  };
}

/** Consumes the event: no browser default, no further listeners. */
function swallow(event: KeyboardEvent): void {
  event.preventDefault();
  event.stopPropagation();
}

/**
 * The capture-phase keydown listener. Resolves the chords pressed so
 * far through a fresh resolver snapshot (rules register as feature
 * chunks load), dispatches KbFound through the command registry, and
 * owns the chord state: pending chords, the waiting and not-a-command
 * statuses, the five-second timer, and the window-blur exit.
 */
export class KeybindingDispatcher extends Disposable {
  private readonly commands: CommandRegistry;
  private readonly contextKeys: ContextKeyService;
  private readonly keybindings: KeybindingsRegistry;
  private readonly status: KeybindingStatusSink;
  private readonly chordPendingKey: ContextKey<boolean>;
  private pending: Chord[] = [];
  private chordTimer: ReturnType<typeof setTimeout> | null = null;
  private statusTimer: ReturnType<typeof setTimeout> | null = null;
  private statusActive = false;

  constructor(deps: KeybindingDispatcherDependencies = {}) {
    super();
    this.commands = deps.commands ?? Commands;
    this.contextKeys = deps.contextKeys ?? getService(CONTEXT_KEY_SERVICE);
    this.keybindings = deps.keybindings ?? KeybindingsRegistry;
    this.status = deps.status ?? createStatusBarSink();
    this.chordPendingKey = this.contextKeys.createKey("chordPending", false);
    const onKeydown = (event: KeyboardEvent): void => this.onKeydown(event);
    document.addEventListener("keydown", onKeydown, true);
    this._register(toDisposable(() => document.removeEventListener("keydown", onKeydown, true)));
    const onBlur = (): void => this.clearPending();
    window.addEventListener("blur", onBlur);
    this._register(toDisposable(() => window.removeEventListener("blur", onBlur)));
    this._register(
      toDisposable(() => {
        this.clearChordTimer();
        this.clearStatusTimer();
      }),
    );
  }

  private onKeydown(event: KeyboardEvent): void {
    const chord = chordFromKeyboardEvent(event);
    if (chord === undefined) {
      // A modifier-only press or an unmapped code is not a chord.
      return;
    }
    const pressed = [...this.pending, chord];
    const resolver = this.keybindings.getResolver();
    const result = resolver.resolve(this.contextKeys, pressed);
    if (result.kind === "KbFound") {
      swallow(event);
      this.clearPending();
      const commandId = result.commandId;
      void this.commands.execute(commandId).catch((error: unknown) => {
        const message = error instanceof Error ? error.message : String(error);
        this.status.showError(`Could not run '${commandId}': ${message}`);
      });
      return;
    }
    if (result.kind === "MoreChordsNeeded") {
      swallow(event);
      this.pending = pressed;
      this.chordPendingKey.set(true);
      this.showStatus(`(${formatKeybinding(pressed)}) was pressed. Waiting for second key of chord...`);
      this.armChordTimer();
      return;
    }
    if (this.pending.length > 0) {
      // The pending prefix plus this key match nothing: abandon the
      // chord, say so for three seconds, and swallow the stray key.
      const rejected = pressed;
      this.pending = [];
      this.chordPendingKey.set(false);
      this.clearChordTimer();
      this.clearStatusTimer();
      swallow(event);
      this.showStatus(`The key combination (${rejected.map((entry) => formatChord(entry)).join(", ")}) is not a command.`);
      this.armStatusTimer();
      return;
    }
    if (resolver.hasRuleForChord(chord)) {
      // Claimed by a rule whose when fails: swallow so the chord never
      // reaches CodeMirror or the webview default.
      swallow(event);
    }
  }

  /** Shows a dispatcher message, replacing any still on the bar. */
  private showStatus(message: string): void {
    this.clearStatusTimer();
    this.status.show(message);
    this.statusActive = true;
  }

  /** Clears the bar only when the dispatcher's message is still up. */
  private clearStatus(): void {
    this.clearStatusTimer();
    if (!this.statusActive) {
      return;
    }
    this.statusActive = false;
    this.status.clear();
  }

  /** Abandons the pending chord: state, context key, timer, status. */
  private clearPending(): void {
    this.clearChordTimer();
    if (this.pending.length === 0) {
      return;
    }
    this.pending = [];
    this.chordPendingKey.set(false);
    this.clearStatus();
  }

  private armChordTimer(): void {
    this.clearChordTimer();
    this.chordTimer = setTimeout(() => {
      this.chordTimer = null;
      this.clearPending();
    }, CHORD_TIMEOUT_MS);
  }

  private clearChordTimer(): void {
    if (this.chordTimer !== null) {
      clearTimeout(this.chordTimer);
      this.chordTimer = null;
    }
  }

  private armStatusTimer(): void {
    this.clearStatusTimer();
    this.statusTimer = setTimeout(() => {
      this.statusTimer = null;
      this.clearStatus();
    }, TRANSIENT_STATUS_MS);
  }

  private clearStatusTimer(): void {
    if (this.statusTimer !== null) {
      clearTimeout(this.statusTimer);
      this.statusTimer = null;
    }
  }
}
