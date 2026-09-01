// The workbench's lucide-backed icon strings, rendered to inline SVG at
// module load. The width/height attributes are part of the contract:
// consumers assign these strings to innerHTML and their CSS sizes against
// the attributes.
import { FolderPlus, Mic, Trash2, createElement } from "lucide";
import type { IconNode } from "lucide";

const svg = (icon: IconNode, size: number): string =>
  createElement(icon, { width: size, height: size }).outerHTML;

/** The Add Folder action, on the header button and the empty-space menu. */
export const ICON_FOLDER_PLUS = svg(FolderPlus, 15);
/** The Remove from Workspace action, on a root row's context menu. */
export const ICON_TRASH_2 = svg(Trash2, 15);
/** The push-to-talk mic, on the agent session's input form. */
export const ICON_MIC = svg(Mic, 16);
