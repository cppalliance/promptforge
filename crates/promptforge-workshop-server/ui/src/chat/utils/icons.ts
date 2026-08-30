import {
	Check,
	ChevronRight,
	Copy,
	Ellipsis,
	EllipsisVertical,
	GitBranch,
	Paperclip,
	Pencil,
	Pin,
	PinOff,
	Settings,
	Trash,
	createElement,
} from "lucide";
import type { IconNode } from "lucide";

// The width/height overrides preserve the dimensions of the hand-pasted SVG
// strings this module used to hold; consumer CSS sizes against them.
const svg = (icon: IconNode, size: number, attrs: Record<string, string | number> = {}): string =>
	createElement(icon, { width: size, height: size, ...attrs }).outerHTML;

export const ICON_COPY = svg(Copy, 15);
export const ICON_CHECK = svg(Check, 15, { stroke: "var(--mur-success)" });
export const ICON_EDIT = svg(Pencil, 15);
export const ICON_SETTINGS = svg(Settings, 20);
export const ICON_PAPERCLIP = svg(Paperclip, 20);
export const ICON_CHEVRON = svg(ChevronRight, 14);
export const ICON_FORK = svg(GitBranch, 15);
export const ICON_MORE_HORIZONTAL = svg(Ellipsis, 16);
export const ICON_MORE_VERTICAL = svg(EllipsisVertical, 16);
export const ICON_PIN = svg(Pin, 15);
export const ICON_PIN_OFF = svg(PinOff, 15);
export const ICON_TRASH = svg(Trash, 15);
