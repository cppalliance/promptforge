import type { AgentRunCollapse, ContentBlock, Message } from "../core/types";

export type FeedItem = Message | FeedAgentRunItem;

export type FeedAgentRunSegment = FeedAgentRunMessagesSegment | FeedAgentRunWorkSegment;

export interface FeedAgentRunMessagesSegment {
	type: "messages";
	id: string;
	messages: readonly Message[];
}

export interface FeedAgentRunWorkSegment {
	type: "work";
	id: string;
	runId: string;
	stepMessages: readonly Message[];
	collapsed: boolean;
	durationMs?: number;
}

export interface FeedAgentRunItem {
	type: "agent_run";
	id: string;
	runId: string;
	userMessage: Message;
	segments: readonly FeedAgentRunSegment[];
	stepMessages: readonly Message[];
	visibleMessages: readonly Message[];
	finalMessage: Message;
	collapsed: boolean;
	durationMs?: number;
}

export interface BuildFeedItemsOptions {
	generatingMessageId: string | null;
	isRunExpanded?: (runId: string) => boolean;
	isWorkSegmentExpanded?: (segmentId: string) => boolean;
	minAgentRunSteps?: number;
	agentRunCollapse?: AgentRunCollapse;
}

const DEFAULT_MIN_AGENT_RUN_STEPS = 1;
const DEFAULT_AGENT_RUN_COLLAPSE: AgentRunCollapse = "machinery";

export function buildFeedItems(messages: readonly Message[], options: BuildFeedItemsOptions): readonly FeedItem[] {
	const items: FeedItem[] = [];
	const minAgentRunSteps = options.minAgentRunSteps ?? DEFAULT_MIN_AGENT_RUN_STEPS;
	const agentRunCollapse = options.agentRunCollapse ?? DEFAULT_AGENT_RUN_COLLAPSE;

	for (let index = 0; index < messages.length; index++) {
		const message = messages[index];

		if (message.role === "user") {
			const runEndIndex = findRunEndIndex(messages, index);
			const runItem =
				runEndIndex - index >= 2
					? buildAgentRunItem(messages, index, runEndIndex, options, minAgentRunSteps, agentRunCollapse)
					: null;

			if (runItem) {
				items.push(runItem);
				index = runEndIndex - 1;
				continue;
			}
		}

		items.push(message);
	}

	return items;
}

export function isAgentRunItem(item: FeedItem): item is FeedAgentRunItem {
	return "type" in item && item.type === "agent_run";
}

export function feedItemType(item: FeedItem): "message" | "agent_run" {
	return isAgentRunItem(item) ? "agent_run" : "message";
}

function findRunEndIndex(messages: readonly Message[], userIndex: number): number {
	const userMessage = messages[userIndex];
	const runId = userMessage.runId;
	let endIndex = userIndex + 1;

	if (runId) {
		while (endIndex < messages.length && messages[endIndex].role !== "user" && messages[endIndex].runId === runId) {
			endIndex++;
		}
	} else {
		while (endIndex < messages.length && messages[endIndex].role !== "user" && !messages[endIndex].runId) {
			endIndex++;
		}
	}

	return endIndex;
}

function buildAgentRunItem(
	messages: readonly Message[],
	userIndex: number,
	runEndIndex: number,
	options: BuildFeedItemsOptions,
	minAgentRunSteps: number,
	agentRunCollapse: AgentRunCollapse,
): FeedAgentRunItem | null {
	let isActiveRun = false;
	if (options.generatingMessageId) {
		for (let i = userIndex; i < runEndIndex; i++) {
			if (messages[i].id !== options.generatingMessageId) continue;
			if (agentRunCollapse !== "machinery") return null;
			isActiveRun = true;
			break;
		}
	}

	const userMessage = messages[userIndex];
	const finalMessageIndex = findFinalAssistantProseIndex(messages, userIndex + 1, runEndIndex);
	if (finalMessageIndex === -1 && !isActiveRun) return null;
	if (agentRunCollapse === "full" && finalMessageIndex !== runEndIndex - 1) return null;

	const runId = userMessage.runId ?? userMessage.id;
	const isWorkSegmentExpanded = (segmentId: string) =>
		isActiveRun || options.isWorkSegmentExpanded?.(segmentId) || options.isRunExpanded?.(runId) || false;
	const segments =
		agentRunCollapse === "full"
			? buildFullSegments(messages, userIndex, finalMessageIndex, runId, isWorkSegmentExpanded)
			: buildMachinerySegments(messages, userIndex, runEndIndex, runId, isWorkSegmentExpanded);
	const stepMessages = flattenStepMessages(segments);
	if (countAgentStepMessages(stepMessages) < minAgentRunSteps) return null;

	const visibleMessages = flattenVisibleMessages(segments);
	const collapsed = segments
		.filter((segment): segment is FeedAgentRunWorkSegment => segment.type === "work")
		.every((segment) => segment.collapsed);
	const finalMessage = messages[finalMessageIndex === -1 ? runEndIndex - 1 : finalMessageIndex];

	return {
		type: "agent_run",
		id: `agent-run:${runId}`,
		runId,
		userMessage,
		segments,
		stepMessages,
		visibleMessages,
		finalMessage,
		collapsed,
		durationMs: calculateRunDuration(userMessage, finalMessage),
	};
}

function buildFullSegments(
	messages: readonly Message[],
	userIndex: number,
	finalMessageIndex: number,
	runId: string,
	isWorkSegmentExpanded: (segmentId: string) => boolean,
): FeedAgentRunSegment[] {
	const stepMessages = buildFullStepMessages(messages, userIndex + 1, finalMessageIndex);
	const finalMachineryBlocks = machineryBlocks(messages[finalMessageIndex]);
	if (finalMachineryBlocks.length > 0) {
		stepMessages.push(createFilteredMessage(messages[finalMessageIndex], finalMachineryBlocks));
	}

	const visibleFinalBlocks = proseBlocks(messages[finalMessageIndex]);
	const segments: FeedAgentRunSegment[] = [];
	if (stepMessages.length > 0) {
		const id = `${runId}:work:0`;
		segments.push({
			type: "work",
			id,
			runId,
			stepMessages,
			collapsed: !isWorkSegmentExpanded(id),
			durationMs: calculateRunDuration(messages[userIndex], messages[finalMessageIndex]),
		});
	}
	if (visibleFinalBlocks.length > 0) {
		segments.push({
			type: "messages",
			id: `${runId}:messages:0`,
			messages: [createFilteredMessage(messages[finalMessageIndex], visibleFinalBlocks)],
		});
	}
	return segments;
}

function buildFullStepMessages(messages: readonly Message[], startIndex: number, finalMessageIndex: number): Message[] {
	const stepMessages: Message[] = [];
	for (let i = startIndex; i < finalMessageIndex; i++) {
		const stepBlocks = messages[i].blocks.filter(isRenderableStepBlock);
		if (stepBlocks.length > 0) stepMessages.push(createFilteredMessage(messages[i], stepBlocks));
	}
	return stepMessages;
}

function buildMachinerySegments(
	messages: readonly Message[],
	userIndex: number,
	runEndIndex: number,
	runId: string,
	isWorkSegmentExpanded: (segmentId: string) => boolean,
): FeedAgentRunSegment[] {
	const segments: FeedAgentRunSegment[] = [];
	let pendingKind: "messages" | "work" | null = null;
	let pendingMessages: Message[] = [];

	const flush = () => {
		if (!pendingKind || pendingMessages.length === 0) return;
		const index = segments.length;
		if (pendingKind === "messages") {
			segments.push({
				type: "messages",
				id: `${runId}:messages:${index}`,
				messages: pendingMessages,
			});
		} else {
			const id = `${runId}:work:${index}`;
			segments.push({
				type: "work",
				id,
				runId,
				stepMessages: pendingMessages,
				collapsed: !isWorkSegmentExpanded(id),
			});
		}
		pendingKind = null;
		pendingMessages = [];
	};

	const append = (kind: "messages" | "work", message: Message, blocks: ContentBlock[]) => {
		if (blocks.length === 0) return;
		if (pendingKind !== kind) flush();
		pendingKind = kind;
		pendingMessages.push(createFilteredMessage(message, blocks));
	};

	for (let i = userIndex + 1; i < runEndIndex; i++) {
		appendMessageChunks(messages[i], append);
	}

	flush();
	moveLeadingReasoningIntoNextWorkSegment(segments);
	applyWorkDurations(segments, messages[userIndex]);
	return segments;
}

function appendMessageChunks(
	message: Message,
	append: (kind: "messages" | "work", message: Message, blocks: ContentBlock[]) => void,
): void {
	if (message.role !== "assistant") {
		append("work", message, message.blocks.filter(isRenderableStepBlock));
		return;
	}

	let currentKind: "messages" | "work" | null = null;
	let currentBlocks: ContentBlock[] = [];

	const flush = () => {
		if (!currentKind || currentBlocks.length === 0) return;
		append(currentKind, message, currentBlocks);
		currentKind = null;
		currentBlocks = [];
	};

	for (const block of message.blocks) {
		const kind = blockKind(block);
		if (!kind) continue;
		if (currentKind !== kind) flush();
		currentKind = kind;
		currentBlocks.push(block);
	}

	flush();
}

function blockKind(block: ContentBlock): "messages" | "work" | null {
	if (isProseBlock(block)) return "messages";
	if (isCollapsibleBlock(block)) return "work";
	return null;
}

function flattenStepMessages(segments: readonly FeedAgentRunSegment[]): Message[] {
	return segments.flatMap((segment) => (segment.type === "work" ? segment.stepMessages : []));
}

function countAgentStepMessages(messages: readonly Message[]): number {
	return new Set(messages.map((message) => message.id)).size;
}

function flattenVisibleMessages(segments: readonly FeedAgentRunSegment[]): Message[] {
	return segments.flatMap((segment) => (segment.type === "messages" ? segment.messages : []));
}

function applyWorkDurations(segments: FeedAgentRunSegment[], userMessage: Message): void {
	let previousVisibleMessage = userMessage;

	for (let i = 0; i < segments.length; i++) {
		const segment = segments[i];
		if (segment.type === "messages") {
			previousVisibleMessage = segment.messages[segment.messages.length - 1] ?? previousVisibleMessage;
			continue;
		}

		const nextVisibleMessage = findNextVisibleMessage(segments, i + 1);
		const lastStepMessage = segment.stepMessages[segment.stepMessages.length - 1];
		if (!lastStepMessage) continue;
		const boundaryDurationMs = nextVisibleMessage
			? calculateRunDuration(previousVisibleMessage, nextVisibleMessage)
			: calculateRunDuration(previousVisibleMessage, lastStepMessage);
		if (boundaryDurationMs !== undefined) segment.durationMs = boundaryDurationMs;
	}
}

function findNextVisibleMessage(segments: readonly FeedAgentRunSegment[], startIndex: number): Message | undefined {
	for (let i = startIndex; i < segments.length; i++) {
		const segment = segments[i];
		if (segment.type === "messages") return segment.messages[0];
	}
	return undefined;
}

function moveLeadingReasoningIntoNextWorkSegment(segments: FeedAgentRunSegment[]): void {
	const firstSegment = segments[0];
	const secondSegment = segments[1];
	if (firstSegment?.type !== "work" || secondSegment?.type !== "messages") return;
	if (!isReasoningOnlyWorkSegment(firstSegment)) return;

	const nextWorkIndex = segments.findIndex((segment, index) => index > 1 && segment.type === "work");
	if (nextWorkIndex === -1) return;

	const nextWorkSegment = segments[nextWorkIndex];
	if (nextWorkSegment.type !== "work") return;

	segments[nextWorkIndex] = {
		...nextWorkSegment,
		stepMessages: [...firstSegment.stepMessages, ...nextWorkSegment.stepMessages],
	};
	segments.shift();
}

function isReasoningOnlyWorkSegment(segment: FeedAgentRunWorkSegment): boolean {
	return segment.stepMessages.every(
		(message) =>
			message.role === "assistant" &&
			message.blocks.length > 0 &&
			message.blocks.every((block) => block.type === "reasoning"),
	);
}

function createFilteredMessage(message: Message, blocks: Message["blocks"]): Message {
	return { ...message, blocks };
}

function findFinalAssistantProseIndex(messages: readonly Message[], startIndex: number, endIndex: number): number {
	for (let i = endIndex - 1; i >= startIndex; i--) {
		const message = messages[i];
		if (message.role === "assistant" && proseBlocks(message).length > 0) return i;
	}

	return -1;
}

function machineryBlocks(message: Message): ContentBlock[] {
	if (message.role !== "assistant") return message.blocks.filter(isRenderableStepBlock);
	return message.blocks.filter(isCollapsibleBlock);
}

function proseBlocks(message: Message): ContentBlock[] {
	if (message.role !== "assistant") return [];
	return message.blocks.filter(isProseBlock);
}

function isProseBlock(block: ContentBlock): boolean {
	switch (block.type) {
		case "text":
			return block.text.trim().length > 0;
		case "artifact":
		case "file":
			return true;
		case "reasoning":
		case "tool_call":
		case "tool_result":
			return false;
	}
}

function isCollapsibleBlock(block: ContentBlock): boolean {
	switch (block.type) {
		case "reasoning":
			return hasVisibleBlock(block);
		case "tool_call":
			return true;
		case "tool_result":
		case "text":
		case "artifact":
		case "file":
			return false;
	}
}

function hasVisibleBlock(block: ContentBlock): boolean {
	switch (block.type) {
		case "text":
			return block.text.trim().length > 0;
		case "reasoning":
			return block.encrypted === true || block.text.trim().length > 0 || Boolean(block.encryptedText);
		case "tool_call":
		case "tool_result":
		case "artifact":
		case "file":
			return true;
	}
}

function isRenderableStepBlock(block: ContentBlock): boolean {
	return block.type !== "tool_result" && hasVisibleBlock(block);
}

function calculateRunDuration(userMessage: Message, finalMessage: Message): number | undefined {
	const startedAt = userMessage.updatedAt ?? userMessage.createdAt;
	const finishedAt = finalMessage.updatedAt ?? finalMessage.createdAt;
	if (startedAt === undefined || finishedAt === undefined) return undefined;
	if (!Number.isFinite(startedAt) || !Number.isFinite(finishedAt) || finishedAt < startedAt) return undefined;
	return finishedAt - startedAt;
}
