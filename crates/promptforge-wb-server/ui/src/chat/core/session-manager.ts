import { uuidv7 } from "../utils/uuid";
import { dropEphemeralMessages, extractPlainText } from "./msg-utils";
import type { Store } from "./store";
import {
	type ChatSession,
	type ChatSessionMeta,
	type ChatState,
	type ChatStorage,
	type ContentBlock,
	MAX_PINNED_SESSIONS,
	type Message,
} from "./types";

interface SessionManagerConfig {
	store: Store<ChatState>;
	storage: ChatStorage;
	isGenerationActive: () => boolean;
	stopActiveGeneration: () => Promise<void>;
}

// Page size for upward message pagination (loadOlderMessages).
const OLDER_MESSAGES_PAGE_SIZE = 100;

export interface ChatSessions {
	loadHistory(): Promise<void>;
	loadMore(): Promise<void>;
	// Loads a page of messages older than the current transcript head and
	// prepends them. No-op unless the storage implements loadOlderMessages and
	// the current session has more history.
	loadOlderMessages(): Promise<void>;
	create(): Promise<void>;
	switch(id: string): Promise<void>;
	delete(id: string): Promise<void>;
	updateTitle(sessionId: string, title: string): Promise<void>;
	updatePinned(sessionId: string, isPinned: boolean): Promise<void>;
}

export class SessionManager implements ChatSessions {
	private store: Store<ChatState>;
	private storage: ChatStorage;
	private isGenerationActive: () => boolean;
	private stopActiveGeneration: () => Promise<void>;
	private activeSessionMeta: ChatSessionMeta | null = null;
	private sessionWriteQueues = new Map<string, Promise<void>>();
	private deletedSessionIds = new Set<string>();
	private isFetchingSessions = false;
	private isFetchingOlder = false;
	private olderCursor: string | null = null;
	private sessionPageCursor: ChatSessionMeta | null = null;
	private switchSeq = 0;

	constructor(config: SessionManagerConfig) {
		this.store = config.store;
		this.storage = config.storage;
		this.isGenerationActive = config.isGenerationActive;
		this.stopActiveGeneration = config.stopActiveGeneration;
	}

	public isDeleted(sessionId: string): boolean {
		return this.deletedSessionIds.has(sessionId);
	}

	public async loadInitial(id: string): Promise<void> {
		await this.loadSession(id, "Chat not found. Started a new one.");
	}

	public async loadHistory(): Promise<void> {
		await this.fetchSessionsPage(false);
	}

	// Call this when the user scrolls to the bottom of the sidebar
	public async loadMore(): Promise<void> {
		await this.fetchSessionsPage(true);
	}

	// Call this when the user scrolls to the top of the transcript.
	public async loadOlderMessages(): Promise<void> {
		if (!this.storage.loadOlderMessages) return;
		if (this.isFetchingOlder || !this.state.hasMoreMessages || !this.olderCursor) return;

		const sessionId = this.state.currentSessionId;
		const cursor = this.olderCursor;

		this.isFetchingOlder = true;
		const seq = this.switchSeq;
		this.store.set({ isLoadingMessages: true });

		try {
			const page = await this.storage.loadOlderMessages(sessionId, cursor, OLDER_MESSAGES_PAGE_SIZE);
			// Drop the result if the user switched/reloaded the session meanwhile.
			if (seq !== this.switchSeq || this.state.currentSessionId !== sessionId) return;

			// Prepend onto the *current* messages (a generation may have appended
			// while we awaited), de-duping any overlap with the existing head.
			const current = this.state.messages;
			const existing = new Set(current.map((m) => m.id));
			const older = page.messages.filter((m) => !existing.has(m.id));
			this.olderCursor = page.nextOlderMessagesCursor ?? null;

			this.store.set({
				messages: [...older, ...current],
				hasMoreMessages: page.hasMore && this.olderCursor !== null,
				isLoadingMessages: false,
			});
		} catch (error) {
			console.error("Failed to load older messages", error);
			if (seq === this.switchSeq && this.state.currentSessionId === sessionId) {
				this.store.set({ isLoadingMessages: false });
			}
		} finally {
			this.isFetchingOlder = false;
		}
	}

	public async create(): Promise<void> {
		if (this.isGenerationActive()) {
			await this.stopActiveGeneration();
		}
		this.startNewSession();
	}

	public async switch(id: string): Promise<void> {
		await this.loadSession(id, "Failed to load chat. Started a new one.");
	}

	public async delete(id: string): Promise<void> {
		const isCurrent = this.state.currentSessionId === id;
		this.deletedSessionIds.add(id);
		this.activeSessionMeta = this.activeSessionMeta?.id === id ? null : this.activeSessionMeta;
		this.store.set({
			sessions: this.state.sessions.filter((s) => s.id !== id),
		});

		try {
			if (isCurrent && this.isGenerationActive()) {
				await this.stopActiveGeneration();
			}

			if (isCurrent && this.state.currentSessionId === id) {
				this.startNewSession();
			}

			await this.enqueueSessionWrite(id, async () => {
				await this.storage.delete(id);
			});
		} catch (error) {
			console.error(`Failed to delete session "${id}"`, error);
		}
	}

	public async persistSessionSnapshot(sessionId: string, messages: Message[]): Promise<boolean> {
		if (this.deletedSessionIds.has(sessionId)) return false;

		const messagesToSave = dropEphemeralMessages(messages);

		try {
			return await this.enqueueSessionWrite(sessionId, async () => {
				if (this.deletedSessionIds.has(sessionId)) return false;

				// Resolve title/isPinned here, not at enqueue time: an earlier queued
				// write (e.g. auto-title's updateTitle) may change them before this
				// operation runs, and a stale snapshot would overwrite that update.
				const existingMeta = this.state.sessions.find((s) => s.id === sessionId);
				const title = existingMeta?.title ?? this.createFallbackTitle(messagesToSave);
				const isPinned =
					existingMeta?.isPinned ??
					(this.activeSessionMeta?.id === sessionId ? this.activeSessionMeta.isPinned : undefined);

				const sessionToSave: ChatSession = {
					id: sessionId,
					title,
					updatedAt: Date.now(),
					...(typeof isPinned === "boolean" ? { isPinned } : {}),
					messages: messagesToSave,
				};

				await this.storage.save(sessionToSave);

				if (this.deletedSessionIds.has(sessionId)) return false;

				const sessionMeta = this.toSessionMeta(sessionToSave);
				if (this.state.currentSessionId === sessionId) {
					this.activeSessionMeta = sessionMeta;
				}
				this.store.set({
					sessions: this.sortSessionMetas([sessionMeta, ...this.state.sessions.filter((s) => s.id !== sessionId)]),
				});

				return true;
			});
		} catch (error) {
			console.error(`Failed to persist session "${sessionId}"`, error);
			return false;
		}
	}

	public async updateTitle(sessionId: string, title: string): Promise<void> {
		if (this.deletedSessionIds.has(sessionId)) return;
		const nextTitle = title.trim();
		if (!nextTitle) return;

		const existingTitle =
			this.state.sessions.find((s) => s.id === sessionId)?.title ??
			(this.activeSessionMeta?.id === sessionId ? this.activeSessionMeta.title : undefined);
		if (existingTitle === nextTitle) return;

		await this.enqueueSessionWrite(sessionId, async () => {
			if (this.deletedSessionIds.has(sessionId)) return;

			if (this.storage.updateMetadata) {
				await this.storage.updateMetadata(sessionId, { title: nextTitle });
			}

			if (this.deletedSessionIds.has(sessionId)) return;
			if (!this.state.sessions.find((s) => s.id === sessionId)) return;

			this.store.set({
				sessions: this.sortSessionMetas(
					this.state.sessions.map((s) => (s.id === sessionId ? { ...s, title: nextTitle } : s)),
				),
			});
			if (this.state.currentSessionId === sessionId && this.activeSessionMeta?.id === sessionId) {
				this.activeSessionMeta = { ...this.activeSessionMeta, title: nextTitle };
			}
		});
	}

	public async updatePinned(sessionId: string, isPinned: boolean): Promise<void> {
		if (this.deletedSessionIds.has(sessionId)) return;

		const current =
			this.state.sessions.find((s) => s.id === sessionId) ??
			(this.activeSessionMeta?.id === sessionId ? this.activeSessionMeta : null);
		if (!current) return;
		if (Boolean(current.isPinned) === isPinned) return;
		if (isPinned && this.countPinnedSessions(sessionId) >= MAX_PINNED_SESSIONS) return;

		await this.enqueueSessionWrite(sessionId, async () => {
			if (this.deletedSessionIds.has(sessionId)) return;

			if (this.storage.updateMetadata) {
				await this.storage.updateMetadata(sessionId, { isPinned });
			}

			if (this.deletedSessionIds.has(sessionId)) return;
			if (!this.state.sessions.find((s) => s.id === sessionId)) return;

			this.store.set({
				sessions: this.sortSessionMetas(this.state.sessions.map((s) => (s.id === sessionId ? { ...s, isPinned } : s))),
			});
			if (this.state.currentSessionId === sessionId && this.activeSessionMeta?.id === sessionId) {
				this.activeSessionMeta = { ...this.activeSessionMeta, isPinned };
			}
		});
	}

	public async close(): Promise<void> {
		if (this.storage.close) {
			await this.storage.close();
		}
	}

	private get state(): ChatState {
		return this.store.get();
	}

	private async fetchSessionsPage(append: boolean): Promise<void> {
		if (this.isFetchingSessions || (append && !this.state.hasMoreSessions)) return;

		this.isFetchingSessions = true;
		this.store.set({ isLoadingSessions: true });

		try {
			const cursor = append ? (this.sessionPageCursor ?? undefined) : undefined;

			const result = await this.storage.loadSessions(20, cursor);
			if (!append) this.sessionPageCursor = null;
			if (result.items.length > 0) {
				this.sessionPageCursor = result.items[result.items.length - 1];
			}

			const resultItems = this.withoutDeletedSessions(result.items);
			const nextSessions = append ? [...this.state.sessions, ...resultItems] : resultItems;

			this.store.set({
				sessions: this.withActiveSessionMeta(nextSessions),
				hasMoreSessions: result.items.length > 0 ? result.hasMore : false,
				isLoadingSessions: false,
			});
		} catch (error) {
			console.error("Failed to load sessions", error);
			this.store.set(
				this.state.error
					? { isLoadingSessions: false }
					: { isLoadingSessions: false, error: { message: "Failed to load chat history." } },
			);
		} finally {
			this.isFetchingSessions = false;
		}
	}

	private async loadSession(id: string, failureMessage: string): Promise<void> {
		if (this.state.currentSessionId === id && !this.state.isLoadingSession) return;

		if (this.isGenerationActive()) {
			await this.stopActiveGeneration();
		}

		const seq = ++this.switchSeq;
		this.activeSessionMeta = null;
		this.olderCursor = null;

		this.store.set({
			currentSessionId: id,
			messages: [],
			isLoadingSession: true,
			hasMoreMessages: false,
			isLoadingMessages: false,
			error: null,
		});

		try {
			const session = await this.storage.loadOne(id);
			if (seq !== this.switchSeq) return; // stale

			// User may have navigated again while this one was loading
			if (this.state.currentSessionId !== id) return;
			if (this.deletedSessionIds.has(id)) throw new Error("Chat not found");

			if (!session) throw new Error("Chat not found");

			this.activeSessionMeta = this.toSessionMeta(session);
			this.olderCursor = session.nextOlderMessagesCursor ?? null;
			this.store.set({
				sessions: this.withActiveSessionMeta(this.state.sessions),
				messages: session.messages,
				isLoadingSession: false,
				hasMoreMessages: Boolean(session.hasMoreMessages && this.olderCursor !== null),
			});
		} catch (error) {
			console.error(`Failed to load session "${id}"`, error);
			if (seq !== this.switchSeq) return;
			if (this.state.currentSessionId !== id) return;

			this.activeSessionMeta = null;
			this.olderCursor = null;
			this.store.set({
				messages: [],
				currentSessionId: uuidv7(),
				isLoadingSession: false,
				hasMoreMessages: false,
				isLoadingMessages: false,
				error: { message: failureMessage },
			});
		}
	}

	private startNewSession(): void {
		this.activeSessionMeta = null;
		this.olderCursor = null;
		this.store.set({
			currentSessionId: uuidv7(),
			messages: [],
			isLoadingSession: false,
			hasMoreMessages: false,
			isLoadingMessages: false,
			error: null,
		});
	}

	private toSessionMeta(session: ChatSession): ChatSessionMeta {
		return {
			id: session.id,
			title: session.title,
			updatedAt: session.updatedAt,
			...(typeof session.isPinned === "boolean" ? { isPinned: session.isPinned } : {}),
		};
	}

	private withActiveSessionMeta(sessions: ChatSessionMeta[]): ChatSessionMeta[] {
		sessions = this.withoutDeletedSessions(sessions);
		const seen = new Set<string>();
		const deduped = sessions.filter((s) => {
			if (seen.has(s.id)) return false;
			seen.add(s.id);
			return true;
		});

		if (!this.activeSessionMeta || this.deletedSessionIds.has(this.activeSessionMeta.id)) {
			return this.sortSessionMetas(deduped);
		}
		if (deduped.some((session) => session.id === this.activeSessionMeta?.id)) {
			return this.sortSessionMetas(deduped);
		}
		return this.sortSessionMetas([this.activeSessionMeta, ...deduped]);
	}

	private sortSessionMetas(sessions: ChatSessionMeta[]): ChatSessionMeta[] {
		return [...sessions].sort((a, b) => {
			const pinnedDelta = Number(Boolean(b.isPinned)) - Number(Boolean(a.isPinned));
			if (pinnedDelta !== 0) return pinnedDelta;
			return b.updatedAt - a.updatedAt || b.id.localeCompare(a.id);
		});
	}

	private countPinnedSessions(exceptSessionId?: string): number {
		return this.state.sessions.filter((session) => session.id !== exceptSessionId && session.isPinned).length;
	}

	private createFallbackTitle(messages: Message[]): string {
		const firstMsg = messages[0];
		if (!firstMsg) return "Empty Chat";

		const text = extractPlainText(firstMsg);
		if (text.trim().length > 0) {
			return text.length > 30 ? `${text.slice(0, 30)}...` : text;
		}

		const fileBlock = firstMsg.blocks.find((b): b is Extract<ContentBlock, { type: "file" }> => b.type === "file");
		if (fileBlock) return `File: ${fileBlock.name || "Upload"}`;

		return "New Chat";
	}

	private enqueueSessionWrite<T>(sessionId: string, operation: () => Promise<T>): Promise<T> {
		const previous = this.sessionWriteQueues.get(sessionId) ?? Promise.resolve();
		const queued = previous.catch(() => undefined).then(operation);
		const tracked = queued.then(
			() => undefined,
			() => undefined,
		);

		this.sessionWriteQueues.set(sessionId, tracked);
		void tracked.finally(() => {
			if (this.sessionWriteQueues.get(sessionId) === tracked) {
				this.sessionWriteQueues.delete(sessionId);
			}
		});

		return queued;
	}

	private withoutDeletedSessions(sessions: ChatSessionMeta[]): ChatSessionMeta[] {
		if (this.deletedSessionIds.size === 0) return sessions;
		return sessions.filter((session) => !this.deletedSessionIds.has(session.id));
	}
}
