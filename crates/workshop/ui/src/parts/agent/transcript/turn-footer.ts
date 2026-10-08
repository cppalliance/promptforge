// The footer under a finished turn: a 28px row with Copy, which puts the
// turn's replies on the clipboard - the markdown source of each reply,
// joined by a blank line. The icon swaps to a check for a moment once the
// copy landed, and the owner raises its confirmation (the toast "Message
// copied to clipboard") through `onCopied`.

import "./transcript.css";

import { ICON_CHECK, ICON_COPY } from "@workshop/look/icons";
import type { IDisposable } from "@workshop/platform/lifecycle";
import { copyToClipboard } from "../../shared/clipboard";

/** How long the check shows before the copy icon returns. */
const CHECK_MS = 2000;

/** The Copy footer of one turn. */
export class TurnFooter implements IDisposable {
  readonly element: HTMLDivElement;

  private readonly button: HTMLButtonElement;
  private text = "";
  private timer: ReturnType<typeof setTimeout> | undefined;

  constructor(private readonly onCopied: () => void) {
    this.element = document.createElement("div");
    this.element.className = "ws-turn-footer";
    this.button = document.createElement("button");
    this.button.type = "button";
    this.button.className = "ws-turn-footer__copy";
    this.button.title = "Copy";
    this.button.setAttribute("aria-label", "Copy");
    // Static strings from @workshop/look, never data.
    this.button.innerHTML = ICON_COPY;
    this.button.addEventListener("click", () => {
      void this.copy();
    });
    this.element.appendChild(this.button);
  }

  /** Sets the reply texts the button copies. */
  update(replies: readonly string[]): void {
    this.text = replies.join("\n\n");
  }

  dispose(): void {
    clearTimeout(this.timer);
  }

  private async copy(): Promise<void> {
    if (!(await copyToClipboard(this.text))) {
      return;
    }
    this.button.innerHTML = ICON_CHECK;
    this.button.dataset["copied"] = "true";
    clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.button.innerHTML = ICON_COPY;
      delete this.button.dataset["copied"];
    }, CHECK_MS);
    this.onCopied();
  }
}
