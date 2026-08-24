import "./edit.css";
import { extractPlainText } from "../../core/msg-utils";
import type { ChatPlugin, Message } from "../../core/types";
import { el, replaceNodes } from "../../utils/dom";
import { ICON_EDIT } from "../../utils/icons";

export interface EditConfig {
	onSave: (messageId: string, newText: string) => void;
}

interface EditState {
	isEditing: boolean;
	editContainer: HTMLElement;
	currentMsg: Message;
}

export function EditPlugin(config: EditConfig): ChatPlugin {
	const stateMap = new WeakMap<HTMLElement, EditState>();

	const ensureState = (parentEl: HTMLElement, msg: Message): EditState => {
		let state = stateMap.get(parentEl);

		if (!state) {
			const editContainer = el("div", "mur-edit-container");
			parentEl.appendChild(editContainer);

			state = {
				isEditing: false,
				editContainer,
				currentMsg: msg,
			};
			stateMap.set(parentEl, state);
		}

		state.currentMsg = msg;
		return state;
	};

	const enterEditMode = (parentEl: HTMLElement, state: EditState) => {
		const msg = state.currentMsg;
		const currentText = extractPlainText(msg);

		const blocksWrapper = parentEl.querySelector(".mur-message-blocks-wrapper") as HTMLElement | null;

		let targetHeight = "auto";
		let targetMinWidth = "100%";

		if (blocksWrapper) {
			targetHeight = Math.max(blocksWrapper.offsetHeight, 24) + "px";
			targetMinWidth = blocksWrapper.offsetWidth + "px";
		}

		state.isEditing = true;
		parentEl.classList.add("mur-editing");

		const textarea = el("textarea", "mur-edit-textarea", { spellcheck: false }) as HTMLTextAreaElement;
		const cancelBtn = el("button", "mur-cancel-edit-btn", { textContent: "Cancel", type: "button" });
		const saveBtn = el("button", "mur-save-edit-btn", { textContent: "Save", type: "button" });
		const controls = el("div", "mur-edit-controls", null, [cancelBtn, saveBtn]);

		replaceNodes(state.editContainer, textarea, controls);

		textarea.style.height = targetHeight;
		textarea.style.minWidth = targetMinWidth;
		textarea.value = currentText;

		textarea.addEventListener("input", () => {
			textarea.style.height = "auto";
			textarea.style.height = textarea.scrollHeight + "px";
		});

		textarea.focus();
		textarea.setSelectionRange(textarea.value.length, textarea.value.length);

		const exitEdit = () => {
			state.isEditing = false;
			parentEl.classList.remove("mur-editing");
			state.editContainer.innerHTML = "";
		};

		cancelBtn.addEventListener("click", exitEdit);

		saveBtn.addEventListener("click", () => {
			const newText = textarea.value.trim();
			if (newText && newText !== currentText) {
				config.onSave(msg.id, newText);
				exitEdit();
			} else {
				exitEdit();
			}
		});

		textarea.addEventListener("keydown", (e) => {
			if (e.key === "Escape") exitEdit();

			if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
				e.preventDefault();
				saveBtn.click();
			}
		});
	};

	return {
		name: "edit",
		getActionButtons: (msg) => {
			if (msg.role !== "user") return [];

			return [
				{
					id: "edit",
					title: "Edit message",
					iconHtml: ICON_EDIT,
					onClick: (ctx) => {
						const state = ensureState(ctx.messageEl, ctx.message);
						enterEditMode(ctx.messageEl, state);
					},
				},
			];
		},
	};
}
