import type { ChatEngine } from "./chat-engine";

export type JsonValue = string | number | boolean | null | { [key: string]: JsonValue } | JsonValue[];

export type ContentBlock =
	| {
			id: string;
			type: "text";
			text: string;
	  }
	| {
			id: string;
			type: "reasoning";
			text: string;
			encrypted?: boolean;
			encryptedText?: string;
	  }
	| {
			id: string;
			type: "tool_call";
			toolCallId: string;
			name: string;
			argsText: string;
			status: "streaming" | "pending" | "running" | "complete" | "error";
	  }
	| {
			id: string;
			type: "tool_result";
			toolCallId: string;
			outputText: string;
			isError?: boolean;
	  }
	| {
			id: string;
			type: "artifact";
			artifactId: string;
			mime: string;
			title?: string;
			content: string;
	  }
	| {
			id: string;
			type: "file";
			mimeType: string;
			name?: string;
			data: string;
	  };

export type Role = "system" | "user" | "assistant" | "tool";

export interface TokenUsage {
	input: number;
	output: number;
	total: number;
	cacheRead?: number;
	cacheWrite?: number;
	details?: JsonValue;
}

export interface Message {
	id: string;
	role: Role;
	blocks: ContentBlock[];
	// Groups messages that belong to the same user-triggered run/turn.
	// Core-generated run ids default to the user message id.
	runId?: string;
	createdAt?: number;
	updatedAt?: number;
	// Used to prevent this message from being sent to the LLM or persisted
	ephemeral?: boolean;
	usage?: TokenUsage;
	// Durable provider/plugin metadata. Must stay JSON-serializable because it
	// can be persisted with chat history.
	meta?: Record<string, JsonValue>;
}

export type FinishReason = "stop" | "length" | "tool_use" | "content_filter" | "error" | "aborted";

/**
 * Normalized streaming events emitted by ChatProvider implementations.
 *
 * Stream contract:
 * - Providers/adapters own upstream quirks and emit Murm message ids.
 * - `runId` is optional on streamed events. When omitted, the engine keeps the
 *   locally generated run id from the user message that started this generation.
 * - The engine creates a temporary empty assistant message before streaming starts.
 *   The first event with a new message id may replace that placeholder id.
 * - `message_start` starts a logical streamed message. A single `streamChat`
 *   call may emit multiple assistant `message_start` events with different ids;
 *   the engine appends each as a new message and continues streaming into it.
 * - Delta/block events should be ordered by message. Once an event starts a new
 *   message id, later deltas are treated as belonging to the active message.
 *   Adapters should not interleave deltas for older messages after switching.
 * - If an adapter cannot emit `message_start`, the first delta/block event with
 *   a new message id can still start an assistant message as a fallback.
 * - `usage` and `finish` apply to the current active streamed message/run.
 */
export type StreamEvent =
	| {
			type: "message_start";
			message: Pick<Message, "id" | "role" | "blocks" | "meta" | "runId" | "createdAt" | "updatedAt">;
	  }
	| {
			type: "text_delta";
			messageId: string;
			blockId: string;
			delta: string;
	  }
	| {
			type: "reasoning_delta";
			messageId: string;
			blockId: string;
			delta: string;
			encrypted?: boolean;
	  }
	| {
			type: "tool_call_start";
			messageId: string;
			block: Extract<ContentBlock, { type: "tool_call" }>;
	  }
	| {
			type: "tool_call_delta";
			messageId: string;
			blockId: string;
			name?: string;
			argsDelta?: string;
			status?: Extract<ContentBlock, { type: "tool_call" }>["status"];
	  }
	| {
			type: "tool_result";
			messageId: string;
			block: Extract<ContentBlock, { type: "tool_result" }>;
	  }
	| {
			type: "artifact";
			messageId: string;
			block: Extract<ContentBlock, { type: "artifact" }>;
	  }
	| {
			type: "usage";
			input: number;
			output: number;
			total?: number;
			cacheRead?: number;
			cacheWrite?: number;
			details?: JsonValue;
	  }
	| {
			type: "finish";
			reason: FinishReason;
	  };

export interface ChatSessionMeta {
	id: string;
	title: string;
	updatedAt: number;
	isPinned?: boolean;
}

export interface ChatSession {
	id: string;
	title: string;
	updatedAt: number;
	isPinned?: boolean;
	messages: Message[];
	// Set by backend-paginated storages whose loadOne returns only the latest
	// window: true when older messages exist and can be fetched via
	// ChatStorage.loadOlderMessages. Storages that load whole sessions omit it.
	hasMoreMessages?: boolean;
	// Opaque backend/storage cursor for the next older page. This is separate
	// from Message.id, which is a UI/wire identity and may not be a storage key.
	nextOlderMessagesCursor?: string;
}

export interface PaginatedSessions {
	items: ChatSessionMeta[];
	hasMore: boolean;
}

export interface ChatState {
	sessions: ChatSessionMeta[];
	hasMoreSessions: boolean;
	currentSessionId: string;
	messages: Message[];
	generatingMessageId: string | null;
	isLoadingSession: boolean;
	isLoadingSessions: boolean;
	// Upward message pagination, parallel to hasMoreSessions/isLoadingSessions.
	// hasMoreMessages stays false unless the storage supports loadOlderMessages
	// and the loaded session reported older history.
	hasMoreMessages: boolean;
	isLoadingMessages: boolean;
	error: { message: string; id?: string } | null;
}

export interface ChatStorage {
	loadSessions(limit: number, cursor?: ChatSessionMeta): Promise<PaginatedSessions>;
	loadOne(id: string): Promise<ChatSession | null>;
	save(session: ChatSession): Promise<void>;
	updateMetadata?(id: string, meta: Partial<ChatSessionMeta>): Promise<void>;
	delete(id: string): Promise<void>;
	/**
	 * Optional upward pagination for backends that return only the latest window
	 * from loadOne. `cursor` is an opaque storage/backend cursor previously
	 * returned as nextOlderMessagesCursor, not a Message.id. Returns a page of
	 * messages oldest-first, plus whether even-older messages remain and the
	 * cursor for the next page. Storages that load whole sessions (the default,
	 * e.g. local IndexedDB) omit this, and the UI never offers "load older".
	 */
	loadOlderMessages?(
		sessionId: string,
		cursor: string,
		limit: number,
	): Promise<{ messages: Message[]; hasMore: boolean; nextOlderMessagesCursor?: string }>;
	close?(): void | Promise<void>;
}

export const MAX_PINNED_SESSIONS = 3;

export type ToolDefinition = Record<string, unknown>;

export interface RequestOptions {
	model?: string;
	temperature?: number;
	top_p?: number;
	max_tokens?: number;
	stream_options?: Record<string, unknown>;
	[key: string]: unknown;
}

export interface ChatRequest {
	messages: Message[];
	instructions?: string;
	tools?: ToolDefinition[];
	options: RequestOptions;
	signal: AbortSignal;
}

export interface ChatRequestDefaults {
	instructions?: string;
	tools?: ToolDefinition[];
	options?: Partial<RequestOptions>;
}

export interface ChatProvider {
	/**
	 * Streams normalized events to the engine. Provider/API failures should reject
	 * this promise; ChatEngine converts rejected provider calls into UI error state.
	 *
	 * Implementations should translate provider-native responses into the StreamEvent
	 * contract above. In particular, they should generate stable message ids when the
	 * upstream provider does not supply them, and should emit a new id for each logical
	 * assistant message produced during the run.
	 */
	streamChat(request: ChatRequest, onEvent: (event: StreamEvent) => void): Promise<void>;

	generateTitle?(request: ChatRequest): Promise<string>;
}

export type CodeHighlighter = (code: string, lang: string) => string | Promise<string>;

export type AgentRunCollapse = "full" | "machinery";

export interface RenderConfig {
	/**
	 * Receives code text from a sanitized code block and returns trusted HTML,
	 * either synchronously or after loading a grammar.
	 * The language is an empty string for code blocks without a language class.
	 * The returned HTML is injected directly, so custom highlighters must escape
	 * any interpolated code text and must not use untrusted highlighter output.
	 */
	highlighter?: CodeHighlighter;
	plugins: ChatPlugin[];
	fullscreen?: boolean;
	agentRunCollapse?: AgentRunCollapse;
	minAgentRunSteps?: number;
	/**
	 * Called when the user scrolls near the top of the transcript and older
	 * messages can be loaded. Wired to ChatEngine.sessions.loadOlderMessages.
	 */
	onReachTop?: () => void;
}

type AnyFn = (...args: never[]) => unknown;
type DeepReadonlyDepth = [never, 0, 1, 2, 3, 4, 5];

export type DeepReadonly<T, Depth extends number = 5> = [Depth] extends [never]
	? T
	: T extends AnyFn
		? T
		: T extends readonly (infer Item)[]
			? readonly DeepReadonly<Item, DeepReadonlyDepth[Depth]>[]
			: T extends object
				? { readonly [K in keyof T]: DeepReadonly<T[K], DeepReadonlyDepth[Depth]> }
				: T;

export interface ReadonlyChatRequest {
	readonly messages: readonly DeepReadonly<Message>[];
	readonly instructions?: string;
	readonly tools?: readonly DeepReadonly<ToolDefinition>[];
	readonly options: DeepReadonly<RequestOptions>;
	readonly signal: AbortSignal;
}

export interface ChatRequestPatch {
	messages?: Message[];
	/**
	 * Omit to keep the accumulated request instructions unchanged.
	 * Return `instructions: undefined` to clear inherited instructions.
	 */
	instructions?: string;
	/**
	 * Omit to keep the accumulated request tools unchanged.
	 * Return `tools: undefined` to clear inherited tools.
	 */
	tools?: ToolDefinition[];
	options?: Partial<RequestOptions>;
}

export interface PluginContext {
	engine: ChatEngine;
	container: HTMLElement;
}

export interface PluginInputContext {
	container: HTMLElement;
	form: HTMLFormElement;
	input: HTMLTextAreaElement;
	requestSubmitStateSync: () => void;
}

export interface MessageActionContext {
	message: Message;
	buttonEl: HTMLElement;
	messageEl: HTMLElement;
	actionId: string;
	pluginName: string;
}

export interface ActionButtonDef {
	id: string;
	title: string;
	iconHtml: string;
	onClick: (ctx: MessageActionContext) => void;
}

export interface BlockRenderContext {
	message: Message;
	messages: readonly Message[];
	blockIndex: number;
}

export interface ChatPlugin {
	name: string;

	/**
	 * When true, the plugin renders its own loading indicator for an empty
	 * generating assistant message, and the core three-dot fallback is
	 * suppressed.
	 */
	ownsEmptyLoadingState?: boolean;

	/**
	 * Fires once when the chat UI initializes.
	 */
	onMount?: (ctx: PluginContext) => void;

	/**
	 * Fires when the chat instance is destroyed.
	 */
	destroy?: () => void;

	/**
	 * Intercept and mutate the payload (messages, options) right before it is sent to the LLM.
	 * To optimize performance, the payload is typed as readonly.
	 * Return a ChatRequestPatch to override specific parts, or void if no changes are needed.
	 * This hook may be async.
	 */
	beforeSubmit?: (request: ReadonlyChatRequest) => ChatRequestPatch | undefined | Promise<ChatRequestPatch | undefined>;

	/**
	 * Fires when the input area mounts. Use to append/prepend custom UI to the form.
	 */
	onInputMount?: (ctx: PluginInputContext) => void;

	/**
	 * Allows the input form to be submitted even if the text area is empty.
	 */
	hasPendingData?: () => boolean;

	/**
	 * Blocks user submission while a plugin is resolving async input state.
	 */
	isSubmitBlocked?: () => boolean;

	/**
	 * Intercept and mutate a newly created user message before it is saved and sent.
	 * This hook must finish synchronously; use beforeSubmit for async request shaping.
	 */
	onUserSubmit?: (msg: Message) => void;

	/**
	 * Declaratively registers static icon buttons for a message action bar.
	 * Called when the action bar is first initialized for a message node.
	 */
	getActionButtons?: (msg: Message) => ActionButtonDef[];

	/**
	 * Intercept the rendering of an individual content block (e.g., text, reasoning, tool_call).
	 * Use this to inject custom UI directly inside a specific block's container.
	 * * @param block The content block data.
	 * @param containerEl The DOM element wrapping this specific block.
	 * @param isGenerating True if the LLM is actively streaming this block.
	 * @param ctx Render-time context for the current block and transcript.
	 * @returns `true` if the plugin handled the render, preventing the core UI from overwriting it.
	 */
	onBlockRender?: (
		block: ContentBlock,
		containerEl: HTMLElement,
		isGenerating: boolean,
		ctx?: BlockRenderContext,
	) => boolean;
}
