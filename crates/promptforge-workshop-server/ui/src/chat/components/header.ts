import type { ChatEngine } from "../core/chat-engine";
import { queryOrThrow } from "../utils/dom";

export interface HeaderProps {
	container: HTMLElement;
	engine: ChatEngine;
	enableSidebar: boolean;
	onOpenSidebar: () => void;
}

export class Header {
	private header: HTMLElement;
	private titleEl: HTMLElement | null;
	private openSidebarBtn?: HTMLButtonElement;
	private unsubscribeTitle: () => void = () => {};

	private onOpenSidebarBound = (event: MouseEvent) => {
		event.stopPropagation();
		this.props.onOpenSidebar();
	};

	constructor(private props: HeaderProps) {
		this.header = queryOrThrow<HTMLElement>(props.container, ".mur-main-header");
		this.titleEl = this.header.querySelector<HTMLElement>(".mur-header-title");

		if (props.enableSidebar) {
			this.openSidebarBtn = queryOrThrow<HTMLButtonElement>(this.header, ".mur-open-sidebar-btn");
			this.openSidebarBtn.addEventListener("click", this.onOpenSidebarBound);
		}

		if (this.titleEl) {
			this.unsubscribeTitle = props.engine.subscribe(
				(state) => state.sessions.find((session) => session.id === state.currentSessionId)?.title ?? "New Chat",
				(title) => this.syncTitle(title),
			);
		}
	}

	public destroy() {
		this.unsubscribeTitle();
		this.openSidebarBtn?.removeEventListener("click", this.onOpenSidebarBound);
	}

	private syncTitle(title: string) {
		if (this.titleEl) {
			this.titleEl.textContent = title;
		}
	}
}
