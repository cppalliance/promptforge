// The dock's Dockview theme: the dark theme with the thin insertion line
// Cursor draws when a tab is dragged between tabs, in place of the filled
// half-tab highlight. The palette itself is the stylesheet's (zones.css
// overrides the theme's variables); only the options that live in the theme
// object, not the CSS, are set here. Tied to dockview 8.3.1's theme shape.

import { themeDark, type DockviewTheme } from "dockview";

export const dockTheme: DockviewTheme = { ...themeDark, dndTabIndicator: "line" };
