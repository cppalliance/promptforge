import { Feed } from "./components/feed";
import { Header } from "./components/header";
import { Input } from "./components/input";
import { type DeleteConfirmation, Sidebar, type SidebarMenuBuilder } from "./components/sidebar";
import { ChatEngine } from "./core/chat-engine";
import type {
	AgentRunCollapse,
	ChatPlugin,
	ChatProvider,
	ChatStorage,
	CodeHighlighter,
	RequestOptions,
} from "./core/types";
import { AppRouter, type RouterConfig } from "./router";
import { el, queryOrThrow } from "./utils/dom";

const PAGE_SCROLL_CLASS = "mur-chat-page-scroll";
let pageScrollAttachCount = 0;

export interface ChatUIConfig {
	container: HTMLElement | string;
	provider: ChatProvider;
	storage: ChatStorage;
	routing?: RouterConfig | boolean;
	titleOptions?: Partial<RequestOptions>;
	titleInstructions?: string;

	/**
	 * Whether Murm UI owns the viewport and uses page-level scrolling on mobile.
	 * Defaults to true. Pass false when rendering inside a containing element.
	 */
	fullscreen?: boolean;
	enableSidebar?: boolean;
	initialSessionId?: string;

	highlighter?: CodeHighlighter;
	plugins?: (chatApi: ChatEngine) => ChatPlugin[];
	agentRunCollapse?: AgentRunCollapse;
	minAgentRunSteps?: number;

	/**
	 * Customizes sidebar item menus. Return the final item list from the provided
	 * defaults; keep side effects inside each item's onClick handler.
	 */
	sidebarMenu?: SidebarMenuBuilder;
	confirmDelete?: DeleteConfirmation;

	/**
	 * Updates the browser window title to match the active chat.
	 * Pass `true` to use the chat title as-is, or a function for custom formatting.
	 */
	updateWindowTitle?: boolean | ((title: string) => string);
}

export class ChatUI {
	public readonly engine: ChatEngine;
	private container: HTMLElement;
	private config: ChatUIConfig;
	private router: AppRouter;

	private inputComponent!: Input;
	private feedComponent!: Feed;
	private headerComponent!: Header;
	private sidebarComponent?: Sidebar;
	private plugins: ChatPlugin[] = [];
	private inputDrafts = new Map<string, string>();
	private unsubscribeWindowTitle: () => void = () => {};
	private usesFullscreenLayout = false;

	private elements!: {
		mainArea: HTMLElement;
		sidebarEl: HTMLElement;
		globalError: HTMLElement;
		globalErrorText: HTMLElement;
		globalErrorCloseBtn: HTMLButtonElement;
	};

	private onMainAreaClickBound = () => this.closeSidebar(true);
	private onSidebarRailClickBound = (event: MouseEvent) => this.handleSidebarRailClick(event);
	private onGlobalErrorCloseBound = (e: MouseEvent) => {
		e.stopPropagation();
		this.engine.clearError();
	};

	constructor(config: ChatUIConfig) {
		this.config = { enableSidebar: true, ...config };
		this.usesFullscreenLayout = this.config.fullscreen !== false;

		let routerConfig: RouterConfig = { type: "hash" };
		if (this.config.routing === false) {
			routerConfig = { type: "none" };
		} else if (typeof this.config.routing === "object") {
			routerConfig = this.config.routing;
		}

		this.router = new AppRouter(routerConfig);

		const el =
			typeof this.config.container === "string" ? document.querySelector(this.config.container) : this.config.container;

		if (!el) throw new Error(`Chat container not found: ${this.config.container}`);
		this.container = el as HTMLElement;
		if (this.usesFullscreenLayout) {
			attachPageScrollClass();
		}

		const initialSessionId = this.config.initialSessionId || this.router.getId() || null;

		this.engine = new ChatEngine({
			provider: this.config.provider,
			storage: this.config.storage,
			initialSessionId,
			titleOptions: this.config.titleOptions,
			titleInstructions: this.config.titleInstructions,
		});

		this.initComponents();
		this.bindEvents();
	}

	public async destroy() {
		this.router.destroy();
		this.unsubscribeWindowTitle();
		this.headerComponent.destroy();
		await this.engine.destroy();

		this.elements.globalErrorCloseBtn.removeEventListener("click", this.onGlobalErrorCloseBound);

		if (this.config.enableSidebar) {
			this.elements.mainArea.removeEventListener("click", this.onMainAreaClickBound);
			this.elements.sidebarEl.removeEventListener("click", this.onSidebarRailClickBound);
		}

		for (const plugin of this.plugins) {
			if (!plugin.destroy) continue;
			try {
				plugin.destroy();
			} catch (error) {
				console.error(`Plugin "${plugin.name}" failed during destroy`, error);
			}
		}

		this.sidebarComponent?.destroy();
		this.feedComponent.destroy();
		this.inputComponent.destroy();
		if (this.usesFullscreenLayout) {
			detachPageScrollClass();
			this.usesFullscreenLayout = false;
		}
	}

	private initComponents() {
		this.plugins = this.config.plugins ? this.config.plugins(this.engine) : [];
		this.engine.registerPlugins(this.plugins);

		this.elements = {} as typeof this.elements;
		this.elements.mainArea = queryOrThrow<HTMLElement>(this.container, ".mur-main-area");
		this.headerComponent = new Header({
			container: this.container,
			engine: this.engine,
			enableSidebar: Boolean(this.config.enableSidebar),
			onOpenSidebar: () => this.openSidebar(),
		});
		this.elements.globalErrorText = el("span", "mur-global-error-text");
		this.elements.globalErrorCloseBtn = el("button", "mur-global-error-close", {
			type: "button",
			textContent: "×",
			title: "Dismiss error",
		});
		this.elements.globalErrorCloseBtn.setAttribute("aria-label", "Dismiss error");
		this.elements.globalError = el(
			"div",
			"mur-global-error",
			{
				hidden: true,
			},
			[this.elements.globalErrorText, this.elements.globalErrorCloseBtn],
		);
		this.elements.globalError.setAttribute("role", "alert");
		this.elements.mainArea.appendChild(this.elements.globalError);

		const pluginCtx = {
			engine: this.engine,
			container: this.container,
		};

		for (const plugin of this.plugins) {
			if (!plugin.onMount) continue;
			try {
				plugin.onMount(pluginCtx);
			} catch (error) {
				console.error(`Plugin "${plugin.name}" failed during onMount`, error);
			}
		}

		this.inputComponent = new Input(
			{
				container: this.container,
				onSubmit: (text) => this.engine.sendMessage(text),
				onStop: () => {
					void this.engine.stopGeneration();
				},
			},
			this.plugins,
		);

		this.feedComponent = new Feed(this.container, {
			highlighter: this.config.highlighter,
			plugins: this.plugins,
			fullscreen: this.usesFullscreenLayout,
			agentRunCollapse: this.config.agentRunCollapse,
			minAgentRunSteps: this.config.minAgentRunSteps,
			onReachTop: () => {
				void this.engine.sessions.loadOlderMessages();
			},
		});

		if (this.config.enableSidebar) {
			this.elements.sidebarEl = queryOrThrow<HTMLElement>(this.container, ".mur-sidebar");
			this.restoreSidebarState();

			this.sidebarComponent = new Sidebar({
				container: this.container,
				engine: this.engine,
				onNewChat: () => {
					void this.engine.sessions.create();
					this.closeSidebar(true);
				},
				onSelectSession: (id) => {
					void this.engine.sessions.switch(id);
					this.closeSidebar(true);
				},
				onLoadMore: () => {
					void this.engine.sessions.loadMore();
				},
				onClose: () => {
					this.closeSidebar(false);
				},
				getSessionHref: (id) => this.router.hrefFor(id),
				sidebarMenu: this.config.sidebarMenu,
				confirmDelete: this.config.confirmDelete,
			});
			void this.engine.sessions.loadHistory();
		}
	}

	private restoreSidebarState() {
		const isDesktopClosed = lsGetItem("mur_sidebar_closed") === "true";
		if (!isDesktopClosed || window.innerWidth <= 768) return;

		const hadAnimatedSidebar = this.container.classList.contains("mur-sidebar-animated");
		if (hadAnimatedSidebar) {
			this.container.classList.remove("mur-sidebar-animated");
		}

		this.container.classList.add("mur-sidebar-closed");

		if (hadAnimatedSidebar) {
			// Commit the restored state before re-enabling sidebar transitions.
			this.elements.sidebarEl.getBoundingClientRect();
			this.container.classList.add("mur-sidebar-animated");
		}
	}

	private bindEvents() {
		this.elements.globalErrorCloseBtn.addEventListener("click", this.onGlobalErrorCloseBound);

		if (this.config.enableSidebar) {
			this.elements.mainArea.addEventListener("click", this.onMainAreaClickBound);
			this.elements.sidebarEl.addEventListener("click", this.onSidebarRailClickBound);
		}

		this.router.listen((id) => {
			if (id) {
				void this.engine.sessions.switch(id);
			} else {
				void this.engine.sessions.create();
			}
		});

		if (this.config.updateWindowTitle) {
			this.unsubscribeWindowTitle = this.engine.subscribe(
				(state) => state.sessions.find((session) => session.id === state.currentSessionId)?.title ?? "New Chat",
				(title) => this.syncWindowTitle(title),
			);
		}

		this.engine.subscribe(
			(state) => state.sessions,
			(sessions) => {
				const state = this.engine.state;
				if (this.config.enableSidebar && this.sidebarComponent) {
					this.sidebarComponent.renderSessions(
						sessions,
						state.currentSessionId,
						state.hasMoreSessions,
						state.isLoadingSessions,
					);
				}
			},
		);

		this.engine.subscribe(
			(state) => (state.hasMoreSessions ? 1 : 0) | (state.isLoadingSessions ? 2 : 0),
			() => {
				const state = this.engine.state;
				if (this.config.enableSidebar && this.sidebarComponent) {
					this.sidebarComponent.renderSessions(
						state.sessions,
						state.currentSessionId,
						state.hasMoreSessions,
						state.isLoadingSessions,
					);
				}
			},
		);

		this.engine.subscribe(
			(state) => state.currentSessionId,
			(currentSessionId) => {
				if (this.config.enableSidebar && this.sidebarComponent) {
					this.sidebarComponent.setActiveSession(currentSessionId);
				}
				this.syncRouterToState();
			},
		);

		this.engine.subscribe(
			(state) =>
				(state.isLoadingSession ? 1 : 0) | (state.error !== null ? 2 : 0) | (state.messages.length > 0 ? 4 : 0),
			() => this.syncRouterToState(),
		);

		this.engine.subscribe(
			(state) => (state.isLoadingSession ? null : state.messages.length === 0),
			(isEmpty) => {
				if (isEmpty !== null) {
					this.container.classList.toggle("mur-chat-empty", isEmpty);
				}
			},
		);

		let prevIsGenerating = false;

		// Feed subscribes to the hot lane because stream chunks are applied via
		// in-place mutation and should not run every normal selector per token.
		this.engine.subscribeHot((state) => {
			const isGenerating = state.generatingMessageId !== null;
			const generationStarted = !prevIsGenerating && isGenerating;

			this.feedComponent.update(
				state.messages,
				state.generatingMessageId,
				state.isLoadingSession,
				generationStarted,
				state.error,
			);
			prevIsGenerating = isGenerating;
		});

		// Older-messages affordance (parallel to the sidebar's load-more state).
		this.engine.subscribe(
			(state) => (state.hasMoreMessages ? 1 : 0) | (state.isLoadingMessages ? 2 : 0),
			() => {
				const state = this.engine.state;
				this.feedComponent.setOlderMessagesState(state.hasMoreMessages, state.isLoadingMessages);
			},
		);

		let inputSessionId = this.engine.state.currentSessionId;
		this.engine.onChange(
			(state) => state.currentSessionId,
			(currentSessionId) => {
				const draft = this.inputComponent.getText();
				if (draft.length > 0) {
					this.inputDrafts.set(inputSessionId, draft);
				} else {
					this.inputDrafts.delete(inputSessionId);
				}

				inputSessionId = currentSessionId;
				this.inputComponent.setText(this.inputDrafts.get(currentSessionId) ?? "");
				this.inputComponent.focus();
			},
		);

		this.engine.subscribe(
			(state) => (state.generatingMessageId ? 2 : 0) | (state.isLoadingSession ? 1 : 0),
			(bits) => {
				const isGenerating = !!(bits & 2);
				const isLoadingSession = !!(bits & 1);

				this.inputComponent.setGeneratingState(isGenerating, isLoadingSession);
			},
		);

		this.engine.subscribe(
			(state) => state.error,
			(error) => this.renderGlobalError(error),
		);
		this.renderGlobalError(this.engine.state.error);
	}

	private syncWindowTitle(title: string) {
		if (!this.config.updateWindowTitle) return;

		document.title = typeof this.config.updateWindowTitle === "function" ? this.config.updateWindowTitle(title) : title;
	}

	private renderGlobalError(error: { message: string; id?: string } | null) {
		if (!error || error.id) {
			this.elements.globalError.hidden = true;
			this.elements.globalErrorText.textContent = "";
			return;
		}

		this.elements.globalErrorText.textContent = error.message;
		this.elements.globalError.hidden = false;
	}

	private syncRouterToState() {
		const state = this.engine.state;
		const currentUrlId = this.router.getId();

		const isSavedSession = state.sessions.some((s) => s.id === state.currentSessionId);
		const shouldHaveUrlId =
			state.messages.length > 0 ||
			isSavedSession ||
			(state.isLoadingSession && currentUrlId === state.currentSessionId);

		const targetId = shouldHaveUrlId ? state.currentSessionId : null;

		if (currentUrlId === targetId) return;

		// If we fell back to an empty chat due to a loading error (e.g., broken link),
		// use replace so we don't trap the user's Back button.
		const isErrorFallback = !shouldHaveUrlId && state.error !== null;
		this.router.setUrl(targetId, isErrorFallback);
	}

	private openSidebar() {
		const isMobile = window.innerWidth <= 768;

		if (isMobile) {
			this.elements.sidebarEl.classList.add("mur-mobile-open");
		} else {
			this.container.classList.remove("mur-sidebar-closed");
			lsSetItem("mur_sidebar_closed", "false");
		}
	}

	private closeSidebar(isNavigation = false) {
		const isMobile = window.innerWidth <= 768;

		if (isMobile) {
			this.elements.sidebarEl.classList.remove("mur-mobile-open");
			return;
		}

		if (isNavigation) return;

		this.container.classList.add("mur-sidebar-closed");
		lsSetItem("mur_sidebar_closed", "true");
	}

	private handleSidebarRailClick(event: MouseEvent) {
		if (window.innerWidth <= 768) return;
		if (!this.container.classList.contains("mur-sidebar-closed")) return;

		const target = event.target;
		if (!(target instanceof Element)) return;
		if (target.closest("button, a, input, textarea, select, [role='button']")) return;

		this.openSidebar();
	}
}

function lsGetItem(key: string): string | null {
	try {
		return localStorage.getItem(key);
	} catch {
		return null;
	}
}

function lsSetItem(key: string, value: string): void {
	try {
		localStorage.setItem(key, value);
	} catch {
		// Ignore
	}
}

function attachPageScrollClass(): void {
	pageScrollAttachCount++;
	document.documentElement.classList.add(PAGE_SCROLL_CLASS);
}

function detachPageScrollClass(): void {
	pageScrollAttachCount = Math.max(0, pageScrollAttachCount - 1);
	if (pageScrollAttachCount === 0) {
		document.documentElement.classList.remove(PAGE_SCROLL_CLASS);
	}
}
