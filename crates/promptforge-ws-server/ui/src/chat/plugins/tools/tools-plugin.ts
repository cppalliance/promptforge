import "./tools.css";
import type { BlockRenderContext, ChatPlugin, Message } from "../../core/types";
import {
	createToolContext,
	type ToolRenderContext,
	type ToolRenderer,
	type ToolResultCache,
	type ToolStatus,
} from "./tool-context";
import { isToolWorking, toolStatus } from "./tool-format";
import { createToolRow, renderToolRow, type ToolRow } from "./tool-row";
import { ToolRunGroup } from "./tool-run-group";

export type { ToolRenderContext, ToolRenderer } from "./tool-context";

export interface ToolsPluginConfig {
	defaultExpanded?: boolean | ((ctx: ToolRenderContext) => boolean);
	maxLabelChars?: number;
	tools?: Record<string, ToolRenderer>;
}

// Per-block state. Consecutive tool_call blocks fold into one collapsible
// run block: the first block of the run (the leader) hosts the run UI in its
// own container; member containers stay hidden while their rows live in the
// leader's preserved log.
interface ToolState {
	containerEl: HTMLElement;
	row: ToolRow;
	ctx?: ToolRenderContext;
	resultCache?: ToolResultCache;
	status: ToolStatus;
	lastIsGenerating: boolean;
	leader: ToolState;
	group: ToolRunGroup | null;
	members: ToolState[];
}

export function ToolsPlugin(config: ToolsPluginConfig = {}): ChatPlugin {
	const stateMap = new WeakMap<HTMLElement, ToolState>();
	// The engine hands every block of a message to the plugins in block order
	// once per state change, with a fresh messages array per change. That
	// identity marks a new render pass, so the open-run table resets exactly
	// when a new pass begins and a member always finds its leader already
	// registered from earlier in the same pass.
	let passMessages: readonly Message[] | null = null;
	const openRuns = new Map<string, ToolState>();

	const beginPass = (renderCtx: BlockRenderContext | undefined): void => {
		const messages = renderCtx?.messages ?? null;
		if (messages === passMessages) return;
		passMessages = messages;
		openRuns.clear();
	};

	const resolveDefaultExpanded = (ctx: ToolRenderContext | undefined): boolean => {
		const setting = config.defaultExpanded;
		if (typeof setting === "function") return ctx ? setting(ctx) : false;
		return setting ?? false;
	};

	const ensureLeaderUi = (state: ToolState): void => {
		if (!state.group) {
			state.group = new ToolRunGroup(() => {
				state.group?.setExpanded(!state.group.isExpanded());
			}, resolveDefaultExpanded(state.ctx));
		}
		if (state.group.rootEl.parentElement !== state.containerEl) {
			state.containerEl.replaceChildren(state.group.rootEl);
		}
		state.containerEl.hidden = false;
		state.group.appendRow(state.row.rootEl);
	};

	const removeMember = (state: ToolState): void => {
		const leader = state.leader;
		if (leader === state) return;
		const index = leader.members.indexOf(state);
		if (index >= 0) leader.members.splice(index, 1);
		state.row.rootEl.remove();
	};

	const promoteToLeader = (state: ToolState): void => {
		removeMember(state);
		state.leader = state;
		state.members = [state];
		ensureLeaderUi(state);
	};

	const joinRun = (state: ToolState, leader: ToolState): void => {
		if (state.leader === state) {
			// Dissolving this state's own run: its members re-evaluate on their
			// own renders later in this same pass and join the run it joins.
			if (state.group) {
				state.group.destroy();
				state.group.rootEl.remove();
				state.group = null;
			}
			state.members = [];
		} else {
			removeMember(state);
		}
		state.leader = leader;
		leader.members.push(state);
		state.containerEl.hidden = true;
		state.containerEl.className = "mur-content-block mur-block-tool_call mur-tool mur-tool-folded";
		ensureLeaderUi(leader);
		leader.group?.appendRow(state.row.rootEl);
	};

	const syncGrouping = (state: ToolState, renderCtx: BlockRenderContext | undefined): void => {
		const message = renderCtx?.message;
		const blockIndex = renderCtx?.blockIndex ?? -1;
		const prevBlock = message && blockIndex > 0 ? message.blocks[blockIndex - 1] : undefined;
		const desiredLeader = prevBlock?.type === "tool_call" && message ? openRuns.get(message.id) : undefined;

		if (desiredLeader && desiredLeader !== state && state.leader !== desiredLeader) {
			joinRun(state, desiredLeader);
		} else if (!desiredLeader && state.leader !== state) {
			promoteToLeader(state);
		} else if (state.leader === state) {
			ensureLeaderUi(state);
		}

		if (state.leader === state && message) {
			openRuns.set(message.id, state);
		}
	};

	const runClassName = (leader: ToolState): string => {
		const members = leader.members;
		const aggregate = members.some((member) => member.status === "error")
			? "error"
			: members.some((member) => isToolWorking(member.status) || member.lastIsGenerating)
				? "running"
				: "complete";
		return `mur-content-block mur-block-tool_call mur-tool mur-tool-run-host mur-tool-run-host--${aggregate}`;
	};

	const syncGroupChrome = (leader: ToolState): void => {
		const group = leader.group;
		if (!group) return;
		leader.containerEl.className = runClassName(leader);

		const members = leader.members;
		const active = [...members].reverse().find((member) => isToolWorking(member.status) || member.lastIsGenerating);
		if (active) {
			group.pushLine(active.row.label);
			return;
		}

		const failed = members.filter((member) => member.status === "error").length;
		const done = members.length - failed;
		const summary =
			failed > 0
				? `${done} ${done === 1 ? "action" : "actions"} completed, ${failed} failed`
				: `${members.length} ${members.length === 1 ? "action" : "actions"} completed`;
		const resting = group.lineText === summary;
		group.pushLine(summary);
		if (!resting) group.announce(summary);
	};

	return {
		name: "tools",
		onBlockRender: (block, containerEl, isGenerating, renderCtx) => {
			if (block.type !== "tool_call") return false;

			beginPass(renderCtx);

			let state = stateMap.get(containerEl);
			if (!state) {
				state = {
					containerEl,
					row: createToolRow(),
					status: "pending",
					lastIsGenerating: false,
					leader: null as unknown as ToolState,
					group: null,
					members: [],
				};
				state.leader = state;
				state.members = [state];
				stateMap.set(containerEl, state);
			}

			const { ctx, cache } = createToolContext(block, renderCtx, isGenerating, state.resultCache);
			state.resultCache = cache;
			state.ctx = ctx;
			state.status = toolStatus(ctx);
			state.lastIsGenerating = isGenerating;

			renderToolRow(state.row, ctx, config.tools?.[block.name], config.maxLabelChars);
			syncGrouping(state, renderCtx);
			syncGroupChrome(state.leader);
			state.leader.group?.maybePin();
			return true;
		},
	};
}
