import "./attachment.css";
import type { ChatPlugin, ContentBlock, PluginInputContext } from "../../core/types";
import { el } from "../../utils/dom";
import { ICON_PAPERCLIP } from "../../utils/icons";
import { uuidv7 } from "../../utils/uuid";

const DEFAULT_ACCEPTED_TYPES = "image/*,text/*,.csv,.json,.md";
const TEXT_FILE_EXTENSIONS = new Set(["csv", "json", "md"]);

type AttachmentState = "processing" | "ready" | "error";

interface AttachmentQueueItem {
	id: string;
	fileName: string;
	mimeType: string;
	state: AttachmentState;
	statusText?: string;
	block?: ContentBlock;
	error?: string;
}

export interface FileHandler {
	accepts: (file: File) => boolean;
	process: (file: File) => Promise<ContentBlock>;
}

export interface AttachmentPluginConfig {
	/** Maximum file size in bytes. Default: 20MB */
	maxFileSize?: number;
	/** Controls the hidden file input accept attribute. */
	acceptedTypes?: string;
	/** Uploads files remotely instead of using built-in local processing. */
	uploadFile?: (file: File) => Promise<{ type: string; data: string; name?: string }>;
	/** Custom parsers for specific file types. First matching handler wins. */
	fileHandlers?: FileHandler[];
	/** Callback when a file exceeds the limit. Native error UI is still shown. */
	onSizeExceeded?: (file: File, maxSize: number) => void;
	/** Callback when a file type is rejected. Native error UI is still shown. */
	onUnsupportedFile?: (file: File) => void;

	/**
	 * A CSS selector defining where the image preview tray should be mounted.
	 * The selector is scoped to the chat container unless previewMountSelectorScope is "document".
	 * If omitted, it will be inserted just before the chat form.
	 */
	previewMountSelector?: string;
	previewMountSelectorScope?: "container" | "document";
}

export function AttachmentPlugin(config?: AttachmentPluginConfig): ChatPlugin {
	const maxSize = config?.maxFileSize ?? 20 * 1024 * 1024;
	const acceptedTypes = config?.acceptedTypes ?? DEFAULT_ACCEPTED_TYPES;

	let queue: AttachmentQueueItem[] = [];

	let fileInput: HTMLInputElement;
	let previewContainer: HTMLElement;
	let attachBtn: HTMLButtonElement;
	let inputContext: PluginInputContext | null = null;
	let dragDepth = 0;
	let destroyed = false;

	const syncSubmitState = () => inputContext?.requestSubmitStateSync();

	const renderPreviews = () => {
		if (!previewContainer) return;
		previewContainer.innerHTML = "";
		previewContainer.hidden = queue.length === 0;

		queue.forEach((item) => {
			const previewItem = el("div", `mur-attachment-preview-item mur-attachment-${item.state}`);
			previewItem.setAttribute("data-attachment-state", item.state);

			if (item.state === "processing") {
				previewItem.appendChild(
					el("div", "mur-file-preview", null, [
						el("span", "mur-attachment-spinner"),
						el("span", "", { textContent: item.statusText ?? "Processing..." }),
					]),
				);
			} else if (item.state === "error") {
				previewItem.appendChild(el("div", "mur-file-preview", { textContent: item.error ?? "Unsupported type" }));
			} else {
				renderReadyPreview(item, previewItem);
			}

			const removeBtn = el("button", "mur-attachment-remove-btn", {
				innerHTML: "×",
				type: "button",
				onclick: () => {
					queue = queue.filter((queuedItem) => queuedItem.id !== item.id);
					renderPreviews();
					syncSubmitState();
				},
			});
			removeBtn.setAttribute("aria-label", `Remove ${item.fileName}`);

			previewItem.appendChild(removeBtn);
			previewContainer.appendChild(previewItem);
		});
	};

	const queueFiles = (files: Iterable<File>) => {
		for (const file of files) {
			void queueFile(file);
		}
	};

	const queueFile = async (file: File) => {
		const item: AttachmentQueueItem = {
			id: uuidv7(),
			fileName: file.name || "Untitled file",
			mimeType: file.type || "application/octet-stream",
			state: "processing",
			statusText: config?.uploadFile ? "Uploading..." : "Processing...",
		};

		queue.push(item);
		renderPreviews();
		syncSubmitState();

		if (file.size > maxSize) {
			updateItemError(item.id, "File too large");
			config?.onSizeExceeded?.(file, maxSize);
			return;
		}

		try {
			const block = await processFile(file);
			updateItemReady(item.id, block);
		} catch (error) {
			const message = error instanceof Error ? error.message : "Unsupported type";
			updateItemError(item.id, message);

			if (message === "Unsupported type") {
				config?.onUnsupportedFile?.(file);
			}
		}
	};

	const updateItemReady = (id: string, block: ContentBlock) => {
		const item = queue.find((queuedItem) => queuedItem.id === id);
		if (!item || destroyed) return;

		item.state = "ready";
		item.block = block;
		item.mimeType = getBlockMimeType(block, item.mimeType);
		item.statusText = undefined;
		item.error = undefined;
		renderPreviews();
		syncSubmitState();
	};

	const updateItemError = (id: string, error: string) => {
		const item = queue.find((queuedItem) => queuedItem.id === id);
		if (!item || destroyed) return;

		item.state = "error";
		item.error = error;
		item.statusText = undefined;
		renderPreviews();
		syncSubmitState();
	};

	const processFile = async (file: File): Promise<ContentBlock> => {
		const handler = config?.fileHandlers?.find((candidate) => candidate.accepts(file));
		if (handler) {
			return handler.process(file);
		}

		if (config?.uploadFile) {
			const uploaded = await config.uploadFile(file);
			return {
				id: uuidv7(),
				type: "file",
				mimeType: uploaded.type,
				name: uploaded.name ?? file.name,
				data: uploaded.data,
			};
		}

		if (file.type.startsWith("image/")) {
			return {
				id: uuidv7(),
				type: "file",
				mimeType: file.type,
				name: file.name,
				data: await readFile(file, "data-url"),
			};
		}

		if (isTextLikeFile(file)) {
			return {
				id: uuidv7(),
				type: "file",
				mimeType: file.type || mimeTypeFromName(file.name),
				name: file.name,
				data: await readFile(file, "text"),
			};
		}

		throw new Error("Unsupported type");
	};

	const onFileInputChange = () => {
		queueFiles(Array.from(fileInput.files || []));
		fileInput.value = "";
	};

	const onDragEnter = (event: DragEvent) => {
		if (!hasDraggedFiles(event)) return;
		event.preventDefault();
		dragDepth++;
		inputContext?.container.classList.add("mur-attachment-drag-active");
	};

	const onDragOver = (event: DragEvent) => {
		if (!hasDraggedFiles(event)) return;
		event.preventDefault();
	};

	const onDragLeave = (event: DragEvent) => {
		if (!hasDraggedFiles(event)) return;
		event.preventDefault();
		dragDepth = Math.max(0, dragDepth - 1);
		if (dragDepth === 0) {
			inputContext?.container.classList.remove("mur-attachment-drag-active");
		}
	};

	const onDrop = (event: DragEvent) => {
		if (!hasDraggedFiles(event)) return;
		event.preventDefault();
		dragDepth = 0;
		inputContext?.container.classList.remove("mur-attachment-drag-active");
		queueFiles(Array.from(event.dataTransfer?.files || []));
	};

	const onPaste = (event: ClipboardEvent) => {
		const files = Array.from(event.clipboardData?.files || []);
		if (files.length === 0) return;

		if (!hasClipboardText(event)) {
			event.preventDefault();
		}
		queueFiles(files);
	};

	return {
		name: "attachments",

		onInputMount: (ctx: PluginInputContext) => {
			inputContext = ctx;
			destroyed = false;
			previewContainer = el("div", "mur-attachment-previews");
			previewContainer.hidden = true;

			fileInput = el("input", "", { type: "file", hidden: true, multiple: true, accept: acceptedTypes });

			attachBtn = el("button", "mur-form-icon-btn", {
				type: "button",
				innerHTML: ICON_PAPERCLIP,
				onclick: () => fileInput.click(),
			});
			attachBtn.setAttribute("aria-label", "Attach files");
			attachBtn.title = "Attach files";

			ctx.form.prepend(attachBtn);
			if (config?.previewMountSelector) {
				const selectorRoot = config.previewMountSelectorScope === "document" ? document : ctx.container;
				const customTarget = selectorRoot.querySelector(config.previewMountSelector);
				if (customTarget) {
					customTarget.appendChild(previewContainer);
				} else {
					console.error(
						`AttachmentPlugin: Could not find element matching previewMountSelector "${config.previewMountSelector}". Image previews will not be visible.`,
					);
				}
			} else {
				ctx.form.before(previewContainer);
			}
			ctx.form.appendChild(fileInput);

			fileInput.addEventListener("change", onFileInputChange);
			ctx.container.addEventListener("dragenter", onDragEnter);
			ctx.container.addEventListener("dragover", onDragOver);
			ctx.container.addEventListener("dragleave", onDragLeave);
			ctx.container.addEventListener("drop", onDrop);
			ctx.input.addEventListener("paste", onPaste);
		},

		hasPendingData: () => queue.some((item) => item.state === "ready" && item.block),

		isSubmitBlocked: () => queue.some((item) => item.state === "processing"),

		onUserSubmit: (msg) => {
			const readyBlocks = queue.flatMap((item) => (item.state === "ready" && item.block ? [item.block] : []));
			if (readyBlocks.length > 0) {
				msg.blocks.unshift(...readyBlocks);
				queue = queue.filter((item) => item.state !== "ready");
				renderPreviews();
				syncSubmitState();
			}
		},

		destroy: () => {
			destroyed = true;
			fileInput?.removeEventListener("change", onFileInputChange);
			inputContext?.container.removeEventListener("dragenter", onDragEnter);
			inputContext?.container.removeEventListener("dragover", onDragOver);
			inputContext?.container.removeEventListener("dragleave", onDragLeave);
			inputContext?.container.removeEventListener("drop", onDrop);
			inputContext?.input.removeEventListener("paste", onPaste);
			inputContext?.container.classList.remove("mur-attachment-drag-active");
			fileInput?.remove();
			attachBtn?.remove();
			previewContainer?.remove();
			queue = [];
			inputContext = null;
			dragDepth = 0;
		},
	};
}

function renderReadyPreview(item: AttachmentQueueItem, previewItem: HTMLElement): void {
	const block = item.block;

	if (block?.type === "file" && block.mimeType.startsWith("image/")) {
		previewItem.appendChild(el("img", "", { src: block.data, alt: block.name ?? item.fileName }));
		return;
	}

	const label = block?.type === "file" ? (block.name ?? item.fileName) : item.fileName;
	previewItem.appendChild(el("div", "mur-file-preview", { textContent: `📄 ${label}` }));
}

function getBlockMimeType(block: ContentBlock, fallback: string): string {
	return block.type === "file" ? block.mimeType : fallback;
}

function hasDraggedFiles(event: DragEvent): boolean {
	const types = event.dataTransfer?.types;
	if (!types) return false;
	return Array.from(types).includes("Files");
}

function hasClipboardText(event: ClipboardEvent): boolean {
	const data = event.clipboardData;
	if (!data) return false;

	const types = Array.from(data.types || []);
	return (
		types.includes("text/plain") ||
		types.includes("text/html") ||
		(typeof data.getData === "function" && data.getData("text/plain").length > 0)
	);
}

function isTextLikeFile(file: File): boolean {
	if (file.type.startsWith("text/") || file.type === "application/json") return true;

	const extension = getFileExtension(file.name);
	return extension !== "" && TEXT_FILE_EXTENSIONS.has(extension);
}

function mimeTypeFromName(fileName: string): string {
	return getFileExtension(fileName) === "json" ? "application/json" : "text/plain";
}

function getFileExtension(fileName: string): string {
	const index = fileName.lastIndexOf(".");
	return index === -1 ? "" : fileName.slice(index + 1).toLowerCase();
}

function readFile(file: File, mode: "data-url" | "text"): Promise<string> {
	return new Promise((resolve, reject) => {
		const reader = new FileReader();

		reader.onload = () => resolve(String(reader.result ?? ""));
		reader.onerror = () => reject(reader.error ?? new Error("Failed to read file"));

		if (mode === "data-url") {
			reader.readAsDataURL(file);
		} else {
			reader.readAsText(file);
		}
	});
}
