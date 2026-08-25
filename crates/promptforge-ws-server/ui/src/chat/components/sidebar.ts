import type { ChatEngine } from "../core/chat-engine";
import { type ChatSessionMeta, MAX_PINNED_SESSIONS } from "../core/types";
import { el, queryOrThrow, replaceNodes } from "../utils/dom";
import { ICON_EDIT, ICON_MORE_VERTICAL, ICON_PIN, ICON_PIN_OFF, ICON_TRASH } from "../utils/icons";
import { closeDropdown, showDropdown } from "./dropdown";

export interface SidebarMenuItem {
	id: string;
	label: string;
	iconHtml?: string;
	danger?: boolean;
	disabled?: boolean;
	onClick: () => void;
}

export type SidebarMenuContext = {
	type: "session";
	session: ChatSessionMeta;
	engine: ChatEngine;
};

export type SidebarMenuBuilder = (
	defaultItems: readonly SidebarMenuItem[],
	ctx: SidebarMenuContext,
) => readonly SidebarMenuItem[];

export type DeleteConfirmation = (session: ChatSessionMeta) => boolean | Promise<boolean>;

export interface SidebarProps {
	container: HTMLElement;
	engine: ChatEngine;
	onNewChat: () => void;
	onSelectSession: (id: string) => void;
	onLoadMore: () => void;
	onClose: () => void;
	getSessionHref: (id: string) => string;
	sidebarMenu?: SidebarMenuBuilder;
	confirmDelete?: DeleteConfirmation;
}

export class Sidebar {
	private sidebar: HTMLElement;
	private content: HTMLElement;
	private newChatBtn?: HTMLButtonElement | null;
	private closeBtn?: HTMLButtonElement | null;
	private pinnedCount = 0;

	private loadMoreTrigger: HTMLElement;
	private observer?: IntersectionObserver;

	private onNewChatBound = () => this.props.onNewChat();
	private onCloseBound = (e: MouseEvent) => {
		e.stopPropagation();
		this.props.onClose();
	};

	constructor(private props: SidebarProps) {
		this.sidebar = queryOrThrow<HTMLElement>(props.container, ".mur-sidebar");
		this.content = queryOrThrow<HTMLElement>(this.sidebar, ".mur-sidebar-content");
		this.newChatBtn = this.sidebar.querySelector(".mur-new-chat-btn");
		this.closeBtn = this.sidebar.querySelector(".mur-close-sidebar-btn");

		this.loadMoreTrigger = el("div", "mur-sidebar-load-more-trigger");

		if (typeof IntersectionObserver !== "undefined") {
			this.observer = new IntersectionObserver(
				(entries) => {
					if (entries[0].isIntersecting) {
						this.props.onLoadMore();
					}
				},
				{
					root: this.content, // Watch scrolling inside the sidebar
					rootMargin: "50px", // Trigger 50px before it actually becomes visible
				},
			);
		}

		this.bindEvents();
	}

	private bindEvents() {
		if (this.newChatBtn) {
			this.newChatBtn.addEventListener("click", this.onNewChatBound);
		}
		if (this.closeBtn) {
			this.closeBtn.addEventListener("click", this.onCloseBound);
		}
	}

	public renderSessions(sessions: ChatSessionMeta[], activeId: string, hasMore: boolean, isLoading = false) {
		closeDropdown();
		this.pinnedCount = sessions.filter((session) => session.isPinned).length;

		if (isLoading && sessions.length === 0) {
			replaceNodes(this.content, el("p", "mur-sidebar-status", { textContent: "Loading chats..." }));
			this.observer?.unobserve(this.loadMoreTrigger);
			return;
		}

		if (sessions.length === 0) {
			replaceNodes(this.content, el("p", "mur-sidebar-status", { textContent: "No past chats." }));
			this.observer?.unobserve(this.loadMoreTrigger);
			return;
		}

		const fragment = document.createDocumentFragment();

		sessions.forEach((session, index) => {
			const isActive = session.id === activeId;
			fragment.appendChild(this.createSessionNode(session, isActive));
			if (session.isPinned && sessions[index + 1] && !sessions[index + 1].isPinned) {
				fragment.appendChild(el("div", "mur-sidebar-pin-divider"));
			}
		});

		if (hasMore) {
			fragment.appendChild(this.loadMoreTrigger);
		}

		replaceNodes(this.content, fragment);

		if (hasMore) {
			this.observer?.observe(this.loadMoreTrigger);
		} else {
			this.observer?.unobserve(this.loadMoreTrigger);
		}
	}

	private createSessionNode(session: ChatSessionMeta, isActive: boolean): HTMLElement {
		const item = el("div", `mur-sidebar-item ${isActive ? "mur-active" : ""} ${session.isPinned ? "mur-pinned" : ""}`);
		item.setAttribute("data-session-id", session.id);

		const link = this.createSessionLink(session, isActive);
		item.appendChild(link);

		const menuItems = this.getSessionMenuItems(session);
		if (menuItems.length > 0) {
			const optionsBtn = el("button", "mur-sidebar-options-btn", {
				type: "button",
				innerHTML: ICON_MORE_VERTICAL,
				title: `Options for "${session.title}"`,
				onclick: (e) => {
					e.preventDefault();
					e.stopPropagation();

					const currentItems = this.getSessionMenuItems(session);
					if (currentItems.length > 0) {
						showDropdown(optionsBtn, currentItems);
					}
				},
			});
			optionsBtn.setAttribute("aria-label", `Options for chat "${session.title}"`);
			item.appendChild(optionsBtn);
		}

		return item;
	}

	private createSessionLink(session: ChatSessionMeta, isActive: boolean): HTMLAnchorElement {
		const link = el("a", "mur-sidebar-item-link", {
			href: this.props.getSessionHref(session.id),
			title: session.title,
			onclick: (e) => {
				e.preventDefault();
				this.props.onSelectSession(session.id);
			},
		});

		if (session.isPinned) {
			const pinIcon = el("span", "mur-sidebar-pin-icon", { innerHTML: ICON_PIN });
			pinIcon.setAttribute("aria-label", "Pinned chat");
			link.appendChild(pinIcon);
		}

		link.appendChild(el("span", "mur-sidebar-item-title", { textContent: session.title }));

		if (isActive) {
			link.setAttribute("aria-current", "page");
		}

		return link;
	}

	private startRename(session: ChatSessionMeta): void {
		const item = Array.from(this.content.querySelectorAll<HTMLElement>(".mur-sidebar-item")).find(
			(node) => node.getAttribute("data-session-id") === session.id,
		);
		const link = item?.querySelector<HTMLAnchorElement>(".mur-sidebar-item-link");
		if (!item || !link) return;

		item.classList.add("mur-renaming");
		const isActive = link.getAttribute("aria-current") === "page";
		const input = el("input", "mur-sidebar-rename-input", {
			type: "text",
			value: session.title,
			ariaLabel: `Rename chat "${session.title}"`,
			onclick: (e) => e.stopPropagation(),
		});

		let finished = false;
		const restore = (title = session.title) => {
			const nextLink = this.createSessionLink({ ...session, title }, isActive);
			item.classList.remove("mur-renaming");
			if (input.isConnected) {
				item.replaceChild(nextLink, input);
			} else {
				const currentLink = item.querySelector<HTMLAnchorElement>(".mur-sidebar-item-link");
				if (currentLink) item.replaceChild(nextLink, currentLink);
			}
		};
		const commit = () => {
			if (finished) return;
			finished = true;
			const title = input.value.trim();
			if (!title || title === session.title) {
				restore();
				return;
			}

			restore(title);
			void this.props.engine.sessions.updateTitle(session.id, title).catch((error) => {
				console.error(`Failed to rename session "${session.id}"`, error);
				restore();
			});
		};
		const cancel = () => {
			if (finished) return;
			finished = true;
			restore();
		};

		input.addEventListener("keydown", (event) => {
			if (event.key === "Enter") {
				event.preventDefault();
				commit();
			} else if (event.key === "Escape") {
				event.preventDefault();
				cancel();
			}
		});
		input.addEventListener("blur", commit);

		item.replaceChild(input, link);
		input.focus();
		input.select();
	}

	private getSessionMenuItems(session: ChatSessionMeta): readonly SidebarMenuItem[] {
		const isPinned = Boolean(session.isPinned);
		const defaultItems: SidebarMenuItem[] = [
			{
				id: "rename",
				label: "Rename",
				iconHtml: ICON_EDIT,
				onClick: () => {
					this.startRename(session);
				},
			},
			{
				id: isPinned ? "unpin" : "pin",
				label: isPinned ? "Unpin" : "Pin",
				iconHtml: isPinned ? ICON_PIN_OFF : ICON_PIN,
				disabled: !isPinned && this.pinnedCount >= MAX_PINNED_SESSIONS,
				onClick: () => {
					void this.props.engine.sessions.updatePinned(session.id, !isPinned).catch((error) => {
						console.error(`Failed to update pinned state for session "${session.id}"`, error);
					});
				},
			},
			{
				id: "delete",
				label: "Delete",
				iconHtml: ICON_TRASH,
				danger: true,
				onClick: () => {
					void this.confirmAndDelete(session);
				},
			},
		];

		return (
			this.props.sidebarMenu?.(defaultItems, { type: "session", session, engine: this.props.engine }) ?? defaultItems
		);
	}

	private async confirmAndDelete(session: ChatSessionMeta): Promise<void> {
		try {
			const confirmed = this.props.confirmDelete
				? await this.props.confirmDelete(session)
				: confirm(`Delete chat "${session.title}"? This cannot be undone.`);

			if (!confirmed) return;
			await this.props.engine.sessions.delete(session.id);
		} catch (error) {
			console.error(`Failed to delete session "${session.id}"`, error);
		}
	}

	public setActiveSession(id: string) {
		const current = this.content.querySelector<HTMLElement>(".mur-sidebar-item.mur-active");
		if (current?.getAttribute("data-session-id") === id) {
			return;
		}

		if (current) {
			current.classList.remove("mur-active");
			current.querySelector(".mur-sidebar-item-link")?.removeAttribute("aria-current");
		}

		const next = Array.from(this.content.querySelectorAll<HTMLElement>(".mur-sidebar-item")).find(
			(item) => item.getAttribute("data-session-id") === id,
		);

		if (next) {
			next.classList.add("mur-active");
			next.querySelector(".mur-sidebar-item-link")?.setAttribute("aria-current", "page");
		}
	}

	public setVisible(isVisible: boolean) {
		this.sidebar.hidden = !isVisible;
	}

	public destroy() {
		closeDropdown();
		this.observer?.disconnect();
		if (this.newChatBtn) {
			this.newChatBtn.removeEventListener("click", this.onNewChatBound);
		}
		if (this.closeBtn) {
			this.closeBtn.removeEventListener("click", this.onCloseBound);
		}
	}
}
