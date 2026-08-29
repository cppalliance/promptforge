import type { Message, RenderConfig } from "../core/types";
import { formatDuration } from "../utils/format";
import { ICON_CHEVRON } from "../utils/icons";
import {
	type FeedAgentRunItem,
	type FeedAgentRunSegment,
	type FeedAgentRunWorkSegment,
	type FeedItem,
	isAgentRunItem,
} from "./feed-items";
import { MessageNode } from "./message-node";
import { TurnFooter } from "./turn-footer";

export interface FeedNodeUpdateContext {
	messages: readonly Message[];
	generatingMessageId: string | null;
	error: { message: string; id?: string } | null;
	onToggleWorkSegment: (segmentId: string) => void;
}

export interface FeedNode {
	type: "message" | "agent_run";
	el: HTMLElement;
	update(item: FeedItem, ctx: FeedNodeUpdateContext): void;
	destroy(): void;
}

export function createFeedNode(item: FeedItem, config: RenderConfig): FeedNode {
	return isAgentRunItem(item) ? new AgentRunFeedNode(item, config) : new MessageFeedNode(item, config);
}

class MessageFeedNode implements FeedNode {
	public readonly type = "message";
	public readonly el: HTMLElement;
	private readonly messageNode: MessageNode;
	private footer?: TurnFooter;

	constructor(message: Message, config: RenderConfig) {
		this.messageNode = new MessageNode(message, config);
		this.el = this.messageNode.el;
		if (message.role === "assistant") {
			this.footer = new TurnFooter(message);
			this.el.appendChild(this.footer.el);
		}
	}

	public update(item: FeedItem, ctx: FeedNodeUpdateContext): void {
		if (isAgentRunItem(item)) return;
		updateMessageNode(this.messageNode, item, ctx);
		if (this.footer) {
			this.footer.update(item);
			this.footer.el.hidden = item.id === ctx.generatingMessageId;
			if (this.el.lastElementChild !== this.footer.el) {
				this.el.appendChild(this.footer.el);
			}
		}
	}

	public destroy(): void {
		this.footer?.destroy();
		this.messageNode.destroy();
	}
}

class AgentRunFeedNode implements FeedNode {
	public readonly type = "agent_run";
	public readonly el = document.createElement("div");

	private readonly segmentNodes = new Map<string, AgentRunSegmentNode>();

	private userNode?: MessageNode;
	private userMessageId?: string;
	private footer?: TurnFooter;

	constructor(
		item: FeedAgentRunItem,
		private readonly config: RenderConfig,
	) {
		this.el.className = "mur-agent-run";
		this.el.dataset.runId = item.runId;
	}

	public update(item: FeedItem, ctx: FeedNodeUpdateContext): void {
		if (!isAgentRunItem(item)) return;

		this.el.dataset.runId = item.runId;

		this.renderUserMessage(item.userMessage, ctx);
		this.renderSegments(item.segments, ctx);
		this.renderFooter(item, ctx);
	}

	public destroy(): void {
		this.footer?.destroy();
		this.userNode?.destroy();
		for (const node of this.segmentNodes.values()) {
			node.destroy();
		}
		this.segmentNodes.clear();
		this.el.remove();
	}

	private renderFooter(item: FeedAgentRunItem, ctx: FeedNodeUpdateContext): void {
		if (!this.footer) {
			this.footer = new TurnFooter(item.finalMessage, item.durationMs);
		}
		this.footer.update(item.finalMessage, item.durationMs);
		if (this.el.lastElementChild !== this.footer.el) {
			this.el.appendChild(this.footer.el);
		}
		this.footer.el.hidden = ctx.generatingMessageId !== null && runContainsMessage(item, ctx.generatingMessageId);
	}

	private renderUserMessage(message: Message, ctx: FeedNodeUpdateContext): void {
		if (!this.userNode || this.userMessageId !== message.id) {
			this.userNode?.destroy();
			this.userNode = new MessageNode(message, this.config);
			this.userMessageId = message.id;
		}

		updateMessageNode(this.userNode, message, ctx);
		if (this.el.firstElementChild !== this.userNode.el) {
			this.el.insertBefore(this.userNode.el, this.el.firstChild);
		}
	}

	private renderSegments(segments: readonly FeedAgentRunSegment[], ctx: FeedNodeUpdateContext): void {
		let previousEl: Element | null = this.userNode?.el ?? null;

		for (const segment of segments) {
			let node = this.segmentNodes.get(segment.id);

			if (!node || node.type !== segment.type) {
				node?.destroy();
				node = createAgentRunSegmentNode(segment, this.config);
				this.segmentNodes.set(segment.id, node);
			}

			if (node.el.parentElement !== this.el || node.el.previousElementSibling !== previousEl) {
				this.el.insertBefore(node.el, previousEl ? previousEl.nextSibling : this.el.firstChild);
			}

			node.update(segment, ctx);
			previousEl = node.el;
		}

		const currentIds = new Set<string>();
		for (const segment of segments) {
			currentIds.add(segment.id);
		}
		for (const [id, node] of this.segmentNodes) {
			if (currentIds.has(id)) continue;
			node.destroy();
			this.segmentNodes.delete(id);
		}
	}
}

interface AgentRunSegmentNode {
	type: FeedAgentRunSegment["type"];
	el: HTMLElement;
	update(segment: FeedAgentRunSegment, ctx: FeedNodeUpdateContext): void;
	destroy(): void;
}

function createAgentRunSegmentNode(segment: FeedAgentRunSegment, config: RenderConfig): AgentRunSegmentNode {
	return segment.type === "work"
		? new AgentRunWorkSegmentNode(segment, config)
		: new AgentRunMessagesSegmentNode(config);
}

class AgentRunMessagesSegmentNode implements AgentRunSegmentNode {
	public readonly type = "messages";
	public readonly el = document.createElement("div");

	private readonly messageNodes = new Map<string, MessageNode>();

	constructor(private readonly config: RenderConfig) {
		this.el.className = "mur-agent-run-messages";
	}

	public update(segment: FeedAgentRunSegment, ctx: FeedNodeUpdateContext): void {
		if (segment.type !== "messages") return;

		for (let index = 0; index < segment.messages.length; index++) {
			const message = segment.messages[index];
			const key = messageNodeKey(message);
			let node = this.messageNodes.get(key);

			if (!node) {
				node = new MessageNode(message, this.config);
				this.messageNodes.set(key, node);
			}

			if (this.el.children[index] !== node.el) {
				this.el.insertBefore(node.el, this.el.children[index]);
			}
			updateMessageNode(node, message, ctx);
		}

		const currentIds = new Set<string>();
		for (const message of segment.messages) {
			currentIds.add(messageNodeKey(message));
		}
		for (const [id, node] of this.messageNodes) {
			if (currentIds.has(id)) continue;
			node.destroy();
			this.messageNodes.delete(id);
		}
	}

	public destroy(): void {
		clearMessageNodes(this.messageNodes);
		this.el.remove();
	}
}

class AgentRunWorkSegmentNode implements AgentRunSegmentNode {
	public readonly type = "work";
	public readonly el = document.createElement("div");

	private readonly summaryEl = document.createElement("button");
	private readonly chevronEl = document.createElement("span");
	private readonly labelEl = document.createElement("span");
	private readonly stepsEl = document.createElement("div");
	private readonly stepNodes = new Map<string, MessageNode>();
	private currentSegmentId?: string;
	private onToggleWorkSegment?: (segmentId: string) => void;

	constructor(
		segment: FeedAgentRunWorkSegment,
		private readonly config: RenderConfig,
	) {
		this.currentSegmentId = segment.id;
		this.el.className = "mur-agent-run-work";
		this.el.dataset.segmentId = segment.id;

		this.summaryEl.type = "button";
		this.summaryEl.className = "mur-agent-run-summary";
		this.summaryEl.addEventListener("click", () => {
			if (this.currentSegmentId) this.onToggleWorkSegment?.(this.currentSegmentId);
		});

		this.chevronEl.className = "mur-agent-run-summary-chevron";
		this.chevronEl.innerHTML = ICON_CHEVRON;
		this.labelEl.className = "mur-agent-run-summary-label";
		this.summaryEl.append(this.chevronEl, this.labelEl);

		this.stepsEl.className = "mur-agent-run-steps";
		this.el.append(this.summaryEl, this.stepsEl);
	}

	public update(segment: FeedAgentRunSegment, ctx: FeedNodeUpdateContext): void {
		if (segment.type !== "work") return;

		this.currentSegmentId = segment.id;
		this.el.dataset.segmentId = segment.id;
		this.onToggleWorkSegment = ctx.onToggleWorkSegment;
		this.renderSummary(segment);
		this.renderSteps(segment, ctx);
	}

	public destroy(): void {
		clearMessageNodes(this.stepNodes);
		this.el.remove();
	}

	private renderSummary(segment: FeedAgentRunWorkSegment): void {
		this.labelEl.textContent = formatWorkSummary(segment);
		this.summaryEl.setAttribute("aria-expanded", String(!segment.collapsed));
	}

	private renderSteps(segment: FeedAgentRunWorkSegment, ctx: FeedNodeUpdateContext): void {
		this.stepsEl.hidden = segment.collapsed;

		if (segment.collapsed) {
			clearMessageNodes(this.stepNodes);
			return;
		}

		for (let index = 0; index < segment.stepMessages.length; index++) {
			const message = segment.stepMessages[index];
			const key = messageNodeKey(message);
			let node = this.stepNodes.get(key);

			if (!node) {
				node = new MessageNode(message, this.config);
				this.stepNodes.set(key, node);
			}

			if (this.stepsEl.children[index] !== node.el) {
				this.stepsEl.insertBefore(node.el, this.stepsEl.children[index]);
			}

			updateMessageNode(node, message, ctx);
		}

		const currentIds = new Set<string>();
		for (const message of segment.stepMessages) {
			currentIds.add(messageNodeKey(message));
		}
		for (const [id, node] of this.stepNodes) {
			if (currentIds.has(id)) continue;
			node.destroy();
			this.stepNodes.delete(id);
		}
	}
}

function updateMessageNode(node: MessageNode, message: Message, ctx: FeedNodeUpdateContext): void {
	const targetError = ctx.error?.id === message.id ? ctx.error.message : null;
	node.update(message, message.id === ctx.generatingMessageId, targetError, ctx.messages);
}

function messageNodeKey(message: Message): string {
	return `${message.id}:${message.blocks.map((block) => block.id).join(",")}`;
}

function runContainsMessage(item: FeedAgentRunItem, messageId: string): boolean {
	if (item.userMessage.id === messageId || item.finalMessage.id === messageId) return true;
	return item.stepMessages.some((message) => message.id === messageId);
}

function clearMessageNodes(nodes: Map<string, MessageNode>): void {
	for (const node of nodes.values()) {
		node.destroy();
	}
	nodes.clear();
}

function formatWorkSummary(segment: FeedAgentRunWorkSegment): string {
	const durationText =
		segment.durationMs === undefined || segment.durationMs <= 0 ? undefined : formatDuration(segment.durationMs);
	const toolCallCount = countToolCalls(segment);

	if (toolCallCount > 0) {
		return durationText
			? `${toolCallCount} ${pluralize("tool call", toolCallCount)}, ${durationText}`
			: `${toolCallCount} ${pluralize("tool call", toolCallCount)}`;
	}

	if (isReasoningOnlySegment(segment)) {
		return durationText ? `Thought for ${durationText}` : "Thought";
	}

	return durationText ? `Worked for ${durationText}` : "Worked";
}

function countToolCalls(segment: FeedAgentRunWorkSegment): number {
	let count = 0;
	for (const message of segment.stepMessages) {
		for (const block of message.blocks) {
			if (block.type === "tool_call") count++;
		}
	}
	return count;
}

function isReasoningOnlySegment(segment: FeedAgentRunWorkSegment): boolean {
	let hasReasoning = false;
	for (const message of segment.stepMessages) {
		for (const block of message.blocks) {
			if (block.type !== "reasoning") return false;
			hasReasoning = true;
		}
	}
	return hasReasoning;
}

function pluralize(label: string, count: number): string {
	return count === 1 ? label : `${label}s`;
}
