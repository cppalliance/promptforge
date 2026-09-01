// The Workshop tree's context menu: a floating list of action buttons
// anchored below (or, at the viewport's bottom edge, above) a trigger
// element. Ported from the vendored murm-ui dropdown, cut to what the
// tree panel uses: no disabled items, no alignment or width options, and
// the open-menu handle lives on an instance the owner disposes instead of
// in module state.
//
// Derived from murm-ui 0.2.0 `components/dropdown.ts`, copyright (c) 2026
// Lev Morozov, MIT License; the full notice is in ui/THIRD_PARTY_NOTICES.md.
import "./dropdown.css";

/** One action row in a dropdown menu. */
export interface DropdownItem {
  label: string;
  iconHtml?: string;
  danger?: boolean;
  onClick: () => void;
}

/**
 * Shows floating menus of action buttons. An instance owns at most one
 * open menu: showing another closes the first, and showing from the same
 * trigger toggles the open one closed. The menu closes on an outside
 * pointer press, Escape (restoring the trigger's focus), Tab, or an item
 * activation; ArrowUp/ArrowDown/Home/End move focus through the items.
 */
export class DropdownMenu {
  private active: { trigger: HTMLElement; close: (restoreFocus?: boolean) => void } | null = null;
  private nextMenuId = 0;

  /** Opens a menu of `items` anchored to `trigger`. */
  show(trigger: HTMLElement, items: readonly DropdownItem[]): void {
    if (this.active !== null) {
      const wasSameTrigger = this.active.trigger === trigger;
      this.active.close(wasSameTrigger);
      if (wasSameTrigger) {
        return;
      }
    }

    const menu = document.createElement("div");
    menu.className = "workshop-dropdown";
    menu.id = `workshop-dropdown-${++this.nextMenuId}`;
    menu.tabIndex = -1;
    menu.setAttribute("role", "menu");
    menu.setAttribute("aria-orientation", "vertical");

    const buttons: HTMLButtonElement[] = [];
    for (const item of items) {
      const button = document.createElement("button");
      button.type = "button";
      button.className =
        item.danger === true
          ? "workshop-dropdown__item workshop-dropdown__item--danger"
          : "workshop-dropdown__item";
      button.setAttribute("role", "menuitem");
      if (item.iconHtml !== undefined) {
        const icon = document.createElement("span");
        icon.className = "workshop-dropdown__icon";
        icon.innerHTML = item.iconHtml;
        button.appendChild(icon);
      }
      const label = document.createElement("span");
      label.className = "workshop-dropdown__label";
      label.textContent = item.label;
      button.appendChild(label);
      button.addEventListener("click", (event) => {
        event.stopPropagation();
        item.onClick();
        this.close();
      });
      buttons.push(button);
      menu.appendChild(button);
    }

    // The trigger's popup wiring is restored on close, so a trigger that
    // carried its own aria state gets it back.
    const previousHasPopup = trigger.getAttribute("aria-haspopup");
    const previousExpanded = trigger.getAttribute("aria-expanded");
    const previousControls = trigger.getAttribute("aria-controls");
    trigger.setAttribute("aria-haspopup", "menu");
    trigger.setAttribute("aria-expanded", "true");
    trigger.setAttribute("aria-controls", menu.id);

    document.body.appendChild(menu);

    // Fixed positioning against the viewport: below the trigger, flipped
    // above when the menu would overflow the bottom edge, right-aligned
    // to the trigger when it would overflow the right edge.
    const triggerRect = trigger.getBoundingClientRect();
    const menuWidth = menu.offsetWidth;
    const menuHeight = menu.offsetHeight;
    if (triggerRect.bottom + 4 + menuHeight > window.innerHeight) {
      menu.style.top = `${triggerRect.top - menuHeight - 4}px`;
    } else {
      menu.style.top = `${triggerRect.bottom + 4}px`;
    }
    if (triggerRect.left + menuWidth > window.innerWidth - 16) {
      menu.style.right = `${window.innerWidth - triggerRect.right}px`;
      menu.style.left = "auto";
    } else {
      menu.style.left = `${triggerRect.left}px`;
      menu.style.right = "auto";
    }

    const onOutsidePointerDown = (event: PointerEvent) => {
      if (!menu.contains(event.target as Node) && !trigger.contains(event.target as Node)) {
        this.close();
      }
    };
    const onEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        this.close(true);
      }
    };
    const focusItem = (offset: number) => {
      if (buttons.length === 0) {
        return;
      }
      const currentIndex = buttons.indexOf(document.activeElement as HTMLButtonElement);
      const nextIndex = currentIndex === -1 ? 0 : (currentIndex + offset + buttons.length) % buttons.length;
      buttons[nextIndex]?.focus();
    };
    const onMenuKeydown = (event: KeyboardEvent) => {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        focusItem(1);
      } else if (event.key === "ArrowUp") {
        event.preventDefault();
        focusItem(-1);
      } else if (event.key === "Home") {
        event.preventDefault();
        buttons[0]?.focus();
      } else if (event.key === "End") {
        event.preventDefault();
        buttons[buttons.length - 1]?.focus();
      } else if (event.key === "Tab") {
        this.close();
      }
    };
    menu.addEventListener("keydown", onMenuKeydown);
    menu.focus();
    document.addEventListener("pointerdown", onOutsidePointerDown);
    document.addEventListener("keydown", onEscape);

    const entry = {
      trigger,
      close: (restoreFocus = false): void => {
        menu.remove();
        document.removeEventListener("pointerdown", onOutsidePointerDown);
        document.removeEventListener("keydown", onEscape);
        restoreAttribute(trigger, "aria-haspopup", previousHasPopup);
        restoreAttribute(trigger, "aria-expanded", previousExpanded);
        restoreAttribute(trigger, "aria-controls", previousControls);
        if (restoreFocus && trigger.isConnected) {
          trigger.focus();
        }
        if (this.active === entry) {
          this.active = null;
        }
      },
    };
    this.active = entry;
  }

  /** Closes the open menu, if any. */
  close(restoreFocus = false): void {
    this.active?.close(restoreFocus);
  }

  /** Closes the open menu; the owner calls this when it tears down. */
  dispose(): void {
    this.close();
  }
}

function restoreAttribute(element: HTMLElement, name: string, value: string | null): void {
  if (value === null) {
    element.removeAttribute(name);
    return;
  }
  element.setAttribute(name, value);
}
