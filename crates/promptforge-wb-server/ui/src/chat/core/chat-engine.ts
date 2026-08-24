import { uuidv7 } from "../utils/uuid";
import { cloneMessages, dropEphemeralMessages } from "./msg-utils";
import { type ChatSessions, SessionManager } from "./session-manager";
import { Store } from "./store";
import { applyStreamEventToState, type StreamReducerEvent } from "./stream-reducer";
import type {
	ChatPlugin,
	ChatProvider,
	ChatRequest,
	ChatRequestDefaults,
	ChatRequestPatch,
	ChatState,
	ChatStorage,
	Message,
	ReadonlyChatRequest,
	RequestOptions,
} from "./types";

export interface ChatEngineConfig {
	provider: ChatProvider;
	storage: ChatStorage;
	initialSessionId?: string | null;
	titleOptions?: Partial<RequestOptions>;
	titleInstructions?: string;
}

interface ActiveGeneration {
	id: string;
	sessionId: string;
	currentMessageId: string;
	controller: AbortController;
	provider: ChatProvider;
	requestDefaults: ChatRequestDefaults;
}

export class ChatEngine {
	private store: Store<ChatState>;
	private readonly sessionManager: SessionManager;
	public readonly sessions: ChatSessions;

	private provider: ChatProvider;
	private plugins: ChatPlugin[] = [];
	private requestDefaults: ChatRequestDefaults = { options: {} };
	private titleOptions: Partial<RequestOptions> = {};
	private titleInstructions?: string;
	private activeGeneration: ActiveGeneration | null = null;
	private autoTitleControllers = new Set<AbortController>();
	private isDestroyed = false;

	constructor(config: ChatEngineConfig) {
		this.provider = config.provider;
		this.titleOptions = this.mergeDefinedOptions({}, config.titleOptions ?? {});
		this.titleInstructions = config.titleInstructions;

		const startingId = config.initialSessionId || uuidv7();

		this.store = new Store<ChatState>({
			sessions: [],
			hasMoreSessions: false,
			currentSessionId: startingId,
			messages: [],
			generatingMessageId: null,
			isLoadingSession: !!config.initialSessionId,
			isLoadingSessions: false,
			hasMoreMessages: false,
			isLoadingMessages: false,
			error: null,
		});
		this.sessionManager = new SessionManager({
			store: this.store,
			storage: config.storage,
			isGenerationActive: () => this.isBusy,
			stopActiveGeneration: () => this.stopGeneration(),
		});
		this.sessions = this.sessionManager;

		if (config.initialSessionId) {
			void this.sessionManager.loadInitial(startingId);
		}
	}

	public registerPlugins(plugins: ChatPlugin[]) {
		this.plugins = plugins;
	}

	public get state(): ChatState {
		return this.store.get();
	}

	public subscribe<U>(selector: (state: ChatState) => U, listener: (selectedState: U) => void): () => void {
		return this.store.subscribe(selector, listener);
	}

	public subscribeHot(listener: (state: ChatState) => void): () => void {
		return this.store.subscribeHot(listener);
	}

	public onChange<U>(selector: (state: ChatState) => U, listener: (selectedState: U) => void): () => void {
		return this.store.onChange(selector, listener);
	}

	private get isBusy() {
		return this.activeGeneration !== null;
	}

	public async setProvider(newProvider: ChatProvider) {
		if (this.isBusy) await this.stopGeneration();
		this.provider = newProvider;
	}

	public clearError() {
		this.store.set({ error: null });
	}

	public sendMessage(content: string): boolean {
		if (this.isBusy || this.state.isLoadingSession) return false;

		const currentMessages = dropEphemeralMessages(this.state.messages);
		const now = Date.now();
		const userMessageId = uuidv7();

		const userMsg: Message = {
			id: userMessageId,
			role: "user",
			blocks: content ? [{ id: uuidv7(), type: "text", text: content }] : [],
			runId: userMessageId,
			createdAt: now,
			updatedAt: now,
		};

		for (const plugin of this.plugins) {
			try {
				plugin.onUserSubmit?.(userMsg);
			} catch (error) {
				console.error(`Plugin "${plugin.name}" failed during onUserSubmit`, error);
			}
		}

		if (userMsg.blocks.length === 0) return false;

		void this.startGeneration([...currentMessages, userMsg]);
		return true;
	}

	public editAndResubmit(messageId: string, newContent: string): boolean {
		if (this.isBusy) return false;

		const currentMessages = dropEphemeralMessages(this.state.messages);
		const targetIndex = currentMessages.findIndex((m) => m.id === messageId);

		if (targetIndex === -1) return false;
		if (currentMessages[targetIndex].role !== "user") return false;

		// Truncate history to remove everything AFTER the edited message
		// and update the edited message itself
		const updatedMessages = currentMessages.slice(0, targetIndex + 1);

		// Preserve non-text blocks (like images/files) and append the edited text
		const preservedBlocks = updatedMessages[targetIndex].blocks.filter((b) => b.type !== "text");
		const newTextBlock = newContent ? [{ id: uuidv7(), type: "text" as const, text: newContent }] : [];
		const finalBlocks = [...preservedBlocks, ...newTextBlock];
		const now = Date.now();

		if (finalBlocks.length === 0) return false;

		updatedMessages[targetIndex] = {
			...updatedMessages[targetIndex],
			blocks: finalBlocks,
			runId: updatedMessages[targetIndex].runId ?? updatedMessages[targetIndex].id,
			createdAt: updatedMessages[targetIndex].createdAt ?? now,
			updatedAt: now,
		};

		void this.startGeneration(updatedMessages);
		return true;
	}

	/**
	 * Completely replaces the current session's message history and attempts to save it to storage.
	 * Useful for clearing history, compacting context, or modifying past messages.
	 */
	public async setMessages(messages: Message[]): Promise<boolean> {
		if (this.isBusy) {
			console.warn("Cannot modify history while the AI is generating a response.");
			return false;
		}

		this.store.set({ messages });
		return await this.persistCurrentSession();
	}

	/**
	 * Sets global default request parameters for outgoing chat requests.
	 * `instructions` and `tools` are request-level model inputs; `options` are provider options.
	 */
	public setRequestDefaults(defaults: Partial<ChatRequestDefaults>) {
		this.requestDefaults = {
			...this.requestDefaults,
			...defaults,
			options: this.mergeDefinedOptions(this.requestDefaults.options ?? {}, defaults.options ?? {}),
		};
	}

	public setTitleOptions(options: Partial<RequestOptions>) {
		this.titleOptions = this.mergeDefinedOptions(this.titleOptions, options);
	}

	public setTitleInstructions(instructions: string | undefined) {
		this.titleInstructions = instructions;
	}

	public async stopGeneration() {
		if (!this.isBusy) return;

		const generation = this.activeGeneration;
		if (!generation) return;

		generation.controller.abort();
		this.applyStreamEvent(generation.id, { type: "finish", reason: "aborted" });
		await this.finalizeGeneration(generation.id, true);
	}

	public async destroy() {
		this.isDestroyed = true;
		this.abortAutoTitles();
		await this.stopGeneration();
		await this.sessionManager.close();
		this.store.clearAllListeners();
	}

	private async startGeneration(contextMessages: Message[]) {
		const generationId = uuidv7();
		const initialMessageId = generationId;
		const sessionId = this.state.currentSessionId;
		const provider = this.provider;
		const controller = new AbortController();
		const signal = controller.signal;
		this.activeGeneration = {
			id: generationId,
			sessionId,
			currentMessageId: initialMessageId,
			controller,
			provider,
			requestDefaults: this.cloneRequestDefaults(),
		};

		// Instantly create an empty assistant message so the UI shows a loading state
		const now = Date.now();
		const runId = findLastUserRunId(contextMessages) ?? initialMessageId;
		const assistantMsg: Message = {
			id: initialMessageId,
			role: "assistant",
			blocks: [],
			runId,
			createdAt: now,
			updatedAt: now,
			ephemeral: true,
		};

		const updatedMessages = [...contextMessages, assistantMsg];

		this.store.set({
			messages: updatedMessages,
			generatingMessageId: initialMessageId,
			error: null,
		});

		let wasAborted = false;
		try {
			const payloadParams = await this.prepareRequestParams(contextMessages, signal);
			if (signal.aborted) {
				wasAborted = true;
				return;
			}

			await provider.streamChat(payloadParams, (event) => {
				if (signal.aborted) return;
				if (event.type === "finish" && event.reason === "aborted") {
					wasAborted = true;
				}
				this.applyStreamEvent(generationId, event);
			});
		} catch (err: unknown) {
			if (signal.aborted) {
				wasAborted = true;
				return;
			}

			const errorMessage =
				err instanceof Error
					? err.message
					: typeof err === "object" && err !== null
						? JSON.stringify(err)
						: String(err);

			this.applyStreamEvent(generationId, { type: "error", message: errorMessage });
		} finally {
			await this.finalizeGeneration(generationId, wasAborted || signal.aborted);
		}
	}

	/**
	 * Applies reducer events without cloning active stream blocks.
	 * @param generationId The ID we generated locally to track the active generation.
	 */
	private applyStreamEvent(generationId: string, event: StreamReducerEvent) {
		const generation = this.activeGeneration;
		if (generation?.id !== generationId) return;

		let currentMessageId = generation.currentMessageId;
		this.store.mutateHot((state) => {
			currentMessageId = applyStreamEventToState(state, generation.currentMessageId, event);
		});
		generation.currentMessageId = currentMessageId;
	}

	private async prepareRequestParams(
		messages: Message[],
		signal: AbortSignal,
		requestDefaults: ChatRequestDefaults = this.requestDefaults,
	): Promise<ChatRequest> {
		const payloadParams: ChatRequest = {
			messages: [...messages],
			instructions: requestDefaults.instructions,
			tools: requestDefaults.tools ? [...requestDefaults.tools] : undefined,
			options: { ...requestDefaults.options },
			signal,
		};

		for (const plugin of this.plugins) {
			if (signal.aborted) return payloadParams;

			if (plugin.beforeSubmit) {
				const request: ReadonlyChatRequest = {
					messages: [...payloadParams.messages],
					instructions: payloadParams.instructions,
					tools: payloadParams.tools ? [...payloadParams.tools] : undefined,
					options: { ...payloadParams.options },
					signal,
				};
				const patch = await plugin.beforeSubmit(request);
				if (signal.aborted) return payloadParams;

				if (patch) {
					if (patch.messages) payloadParams.messages = patch.messages;
					if (hasPatchField(patch, "instructions")) {
						payloadParams.instructions = patch.instructions;
					}
					if (hasPatchField(patch, "tools")) {
						payloadParams.tools = patch.tools ? [...patch.tools] : undefined;
					}
					if (patch.options) {
						payloadParams.options = this.mergeDefinedOptions(payloadParams.options, patch.options) as RequestOptions;
					}
				}
			}
		}

		payloadParams.messages = dropEphemeralMessages(payloadParams.messages);

		return payloadParams;
	}

	private async finalizeGeneration(generationId: string, wasAborted: boolean = false) {
		const generation = this.activeGeneration;
		if (generation?.id !== generationId) return;
		this.activeGeneration = null;

		if (wasAborted) {
			this.removeAbortedEphemeralMessage(generation.currentMessageId);
		}

		if (this.state.generatingMessageId !== null) {
			this.store.set({ generatingMessageId: null });
		}

		try {
			const finalMessages = cloneMessages(this.state.messages);
			const persistentMessages = dropEphemeralMessages(finalMessages);
			const hasError = this.state.error !== null;
			const saved = await this.sessionManager.persistSessionSnapshot(generation.sessionId, finalMessages);

			if (!saved) return;

			// Auto-title trigger
			if (!hasError && !wasAborted && generation.provider.generateTitle) {
				const assistantRepliesCount = persistentMessages.filter(
					(m) => m.role === "assistant" && m.blocks.length > 0,
				).length;

				if (assistantRepliesCount === 1) {
					void this.triggerAutoTitle(
						generation.sessionId,
						persistentMessages,
						generation.provider,
						generation.requestDefaults,
					);
				}
			}
		} catch (error) {
			console.error("Failed to finalize stream", error);
		}
	}

	private removeAbortedEphemeralMessage(pendingId: string): void {
		const pendingMessage = this.state.messages.find((m) => m.id === pendingId);
		if (!pendingMessage?.ephemeral) return;

		this.store.set({
			messages: this.state.messages.filter((m) => m.id !== pendingId),
		});
	}

	private async persistCurrentSession(): Promise<boolean> {
		const { currentSessionId, messages } = this.store.get();
		return await this.sessionManager.persistSessionSnapshot(currentSessionId, cloneMessages(messages));
	}

	private async triggerAutoTitle(
		sessionId: string,
		messages: Message[],
		provider: ChatProvider,
		requestDefaults: ChatRequestDefaults,
	) {
		if (this.isDestroyed || this.sessionManager.isDeleted(sessionId)) return;

		const controller = new AbortController();
		this.autoTitleControllers.add(controller);

		try {
			const payloadMessages = dropEphemeralMessages(messages);
			const payloadOptions = { ...requestDefaults.options, ...this.titleOptions };
			const titleRequest: ChatRequest = {
				messages: payloadMessages,
				instructions: this.titleInstructions,
				options: payloadOptions,
				signal: controller.signal,
			};

			const smartTitle = await provider.generateTitle!(titleRequest);
			if (!smartTitle) return;
			if (controller.signal.aborted || this.isDestroyed || this.sessionManager.isDeleted(sessionId)) return;

			await this.sessionManager.updateTitle(sessionId, smartTitle);
		} catch (e) {
			if (controller.signal.aborted) return;
			console.error("Failed to auto-generate title", e);
		} finally {
			this.autoTitleControllers.delete(controller);
		}
	}

	private abortAutoTitles(): void {
		for (const controller of this.autoTitleControllers) {
			controller.abort();
		}
		this.autoTitleControllers.clear();
	}

	private mergeDefinedOptions(base: Partial<RequestOptions>, patch: Partial<RequestOptions>): Partial<RequestOptions> {
		const next: Partial<RequestOptions> = { ...base };
		for (const [key, value] of Object.entries(patch)) {
			if (value === undefined) {
				delete next[key];
			} else {
				next[key] = value;
			}
		}
		return next;
	}

	private cloneRequestDefaults(defaults: ChatRequestDefaults = this.requestDefaults): ChatRequestDefaults {
		return {
			instructions: defaults.instructions,
			tools: defaults.tools ? [...defaults.tools] : undefined,
			options: { ...defaults.options },
		};
	}
}

function findLastUserRunId(messages: readonly Message[]): string | undefined {
	for (let i = messages.length - 1; i >= 0; i--) {
		const message = messages[i];
		if (message.role === "user") return message.runId ?? message.id;
	}

	return undefined;
}

function hasPatchField(patch: ChatRequestPatch, key: keyof ChatRequestPatch): boolean {
	// biome-ignore lint/suspicious/noPrototypeBuiltins: Object.hasOwn is ES2022, but core targets ES2018.
	return Object.prototype.hasOwnProperty.call(patch, key);
}
