import { el } from "../utils/dom";

export interface DropdownItem {
	id: string;
	label: string;
	iconHtml?: string;
	danger?: boolean;
	disabled?: boolean;
	onClick: () => void;
}

export interface DropdownOptions {
	align?: "left" | "right";
	width?: string;
}

let activeDropdown: { menu: HTMLElement; trigger: HTMLElement; cleanup: (restoreFocus?: boolean) => void } | null =
	null;
let nextDropdownId = 0;

export function showDropdown(trigger: HTMLElement, items: readonly DropdownItem[], options: DropdownOptions = {}) {
	if (activeDropdown) {
		const wasSameTrigger = activeDropdown.trigger === trigger;
		activeDropdown.cleanup(wasSameTrigger);
		if (wasSameTrigger) return;
	}

	const menu = el("div", "mur-dropdown-menu");
	const menuId = `mur-dropdown-${++nextDropdownId}`;
	menu.id = menuId;
	menu.tabIndex = -1;
	menu.setAttribute("role", "menu");
	menu.setAttribute("aria-orientation", "vertical");
	if (options.width) menu.style.width = options.width;

	items.forEach((item) => {
		const btnClass = item.danger ? "mur-dropdown-item mur-danger" : "mur-dropdown-item";
		const btn = el("button", btnClass, {
			type: "button",
			disabled: item.disabled,
			onclick: (e) => {
				e.stopPropagation();
				if (!item.disabled) {
					item.onClick();
					closeDropdown();
				}
			},
		});
		btn.setAttribute("role", "menuitem");

		if (item.iconHtml) {
			btn.appendChild(el("span", "mur-dropdown-icon", { innerHTML: item.iconHtml }));
		}
		btn.appendChild(el("span", "mur-dropdown-label", { textContent: item.label }));

		menu.appendChild(btn);
	});
	const enabledItems = Array.from(menu.querySelectorAll<HTMLButtonElement>(".mur-dropdown-item:not(:disabled)"));

	const appContainer = trigger.closest(".mur-app") || document.body;
	appContainer.appendChild(menu);

	const previousAriaHasPopup = trigger.getAttribute("aria-haspopup");
	const previousAriaExpanded = trigger.getAttribute("aria-expanded");
	const previousAriaControls = trigger.getAttribute("aria-controls");
	trigger.setAttribute("aria-haspopup", "menu");
	trigger.setAttribute("aria-expanded", "true");
	trigger.setAttribute("aria-controls", menuId);

	const triggerRect = trigger.getBoundingClientRect();
	const appRect = appContainer.getBoundingClientRect();
	const menuWidth = menu.offsetWidth;
	const menuHeight = menu.offsetHeight;
	const top = triggerRect.bottom - appRect.top;
	const left = triggerRect.left - appRect.left;

	if (top + 4 + menuHeight > appRect.height) {
		menu.style.top = `${triggerRect.top - appRect.top - menuHeight - 4}px`;
	} else {
		menu.style.top = `${top + 4}px`;
	}

	const alignRightEdge = options.align === "right" || (!options.align && left + menuWidth > appRect.width - 16);

	if (alignRightEdge) {
		const rightOffset = appRect.right - triggerRect.right;
		menu.style.right = `${rightOffset}px`;
		menu.style.left = "auto";
	} else {
		menu.style.left = `${left}px`;
		menu.style.right = "auto";
	}

	const handleOutsidePointerDown = (e: PointerEvent) => {
		if (!menu.contains(e.target as Node) && !trigger.contains(e.target as Node)) {
			closeDropdown();
		}
	};

	const handleEsc = (e: KeyboardEvent) => {
		if (e.key === "Escape") {
			e.preventDefault();
			closeDropdown(true);
		}
	};

	const focusMenuItem = (offset: number) => {
		if (enabledItems.length === 0) return;

		const currentIndex = enabledItems.indexOf(document.activeElement as HTMLButtonElement);
		const nextIndex = currentIndex === -1 ? 0 : (currentIndex + offset + enabledItems.length) % enabledItems.length;
		enabledItems[nextIndex].focus();
	};

	const handleMenuKeydown = (e: KeyboardEvent) => {
		if (e.key === "ArrowDown") {
			e.preventDefault();
			focusMenuItem(1);
		} else if (e.key === "ArrowUp") {
			e.preventDefault();
			focusMenuItem(-1);
		} else if (e.key === "Home") {
			e.preventDefault();
			enabledItems[0]?.focus();
		} else if (e.key === "End") {
			e.preventDefault();
			enabledItems[enabledItems.length - 1]?.focus();
		} else if (e.key === "Tab") {
			closeDropdown();
		}
	};
	menu.addEventListener("keydown", handleMenuKeydown);
	menu.focus();

	document.addEventListener("pointerdown", handleOutsidePointerDown);
	document.addEventListener("keydown", handleEsc);

	const cleanup = (restoreFocus = false) => {
		menu.remove();
		menu.removeEventListener("keydown", handleMenuKeydown);
		document.removeEventListener("pointerdown", handleOutsidePointerDown);
		document.removeEventListener("keydown", handleEsc);
		restoreAttribute(trigger, "aria-haspopup", previousAriaHasPopup);
		restoreAttribute(trigger, "aria-expanded", previousAriaExpanded);
		restoreAttribute(trigger, "aria-controls", previousAriaControls);
		if (restoreFocus && trigger.isConnected) {
			trigger.focus();
		}
		if (activeDropdown?.menu === menu) activeDropdown = null;
	};

	activeDropdown = { menu, trigger, cleanup };
}

export function closeDropdown(restoreFocus = false) {
	if (activeDropdown) {
		activeDropdown.cleanup(restoreFocus);
	}
}

function restoreAttribute(element: HTMLElement, name: string, value: string | null) {
	if (value === null) {
		element.removeAttribute(name);
		return;
	}

	element.setAttribute(name, value);
}
