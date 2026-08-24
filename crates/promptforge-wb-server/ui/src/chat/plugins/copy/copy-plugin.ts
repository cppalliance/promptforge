import { extractPlainText } from "../../core/msg-utils";
import type { ChatPlugin } from "../../core/types";
import { ICON_CHECK, ICON_COPY } from "../../utils/icons";

export function CopyPlugin(): ChatPlugin {
	return {
		name: "copy",
		getActionButtons: (msg) => {
			if (msg.role !== "assistant") return [];
			if (typeof navigator === "undefined" || !navigator.clipboard) return [];
			if (!extractPlainText(msg).trim()) return [];

			return [
				{
					id: "copy",
					title: "Copy message",
					iconHtml: ICON_COPY,
					onClick: async ({ message, buttonEl }) => {
						try {
							const textToCopy = extractPlainText(message);
							await navigator.clipboard.writeText(textToCopy);
							buttonEl.innerHTML = ICON_CHECK;
							setTimeout(() => {
								if (buttonEl.isConnected) {
									buttonEl.innerHTML = ICON_COPY;
								}
							}, 2000);
						} catch {
							// Ignore
						}
					},
				},
			];
		},
	};
}
