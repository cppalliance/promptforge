import type { ChatSession, ChatSessionMeta, ChatStorage, Message, PaginatedSessions } from "../types";

export class RemoteStorageError extends Error {
	constructor(
		action: string,
		public readonly status: number,
		public readonly url: string,
		public readonly responseBody: string,
	) {
		const bodyExcerpt = responseBody ? `: ${responseBody.slice(0, 500)}` : "";
		super(`${action} (${status}) at ${url}${bodyExcerpt}`);
		this.name = "RemoteStorageError";
	}
}

export interface RemoteStorageOptions {
	/**
	 * Limits the number of messages sent during a save() operation.
	 * WARNING: If you use this, your backend must upsert messages rather than
	 * overwrite the entire chat record when the partial save header is present.
	 */
	saveLimit?: number;
}

export class RemoteStorage implements ChatStorage {
	constructor(
		private baseUrl: string,
		private getToken: () => string | null,
		private options?: RemoteStorageOptions,
	) {}

	private get headers(): Record<string, string> {
		const token = this.getToken();
		return {
			"Content-Type": "application/json",
			...(token ? { Authorization: `Bearer ${token}` } : {}),
		};
	}

	private getPath(suffix = ""): string {
		const base = this.baseUrl.replace(/\/+$/, "");
		return `${base}/chats${suffix}`;
	}

	async loadSessions(limit: number, cursor?: ChatSessionMeta): Promise<PaginatedSessions> {
		const params = new URLSearchParams({ limit: limit.toString() });
		if (cursor) {
			params.append("cursor", cursor.updatedAt.toString());
			params.append("cursorId", cursor.id);
			params.append("cursorPinned", String(Boolean(cursor.isPinned)));
		}

		const url = `${this.getPath()}?${params.toString()}`;
		const res = await fetch(url, { headers: this.headers });
		await this.assertOk(res, "Failed to load chats", url);
		return res.json();
	}

	async loadOne(id: string): Promise<ChatSession | null> {
		const url = this.getPath(`/${encodeURIComponent(id)}`);
		const res = await fetch(url, {
			headers: this.headers,
		});
		if (res.status === 404) return null;
		await this.assertOk(res, "Failed to load chat", url);
		return res.json();
	}

	async save(session: ChatSession): Promise<void> {
		const limit = this.getSaveLimit();
		let payload = session;
		const headers = this.headers;

		if (limit && session.messages.length > limit) {
			payload = {
				...session,
				messages: session.messages.slice(-limit),
			};
			headers["X-Murm-Save-Mode"] = "partial";
		}

		const url = this.getPath(`/${encodeURIComponent(session.id)}`);
		const res = await fetch(url, {
			method: "PUT",
			headers,
			body: JSON.stringify(payload),
		});
		await this.assertOk(res, "Failed to save chat", url);
	}

	private getSaveLimit(): number | null {
		const limit = this.options?.saveLimit;
		if (typeof limit !== "number" || !Number.isFinite(limit)) return null;

		const wholeLimit = Math.floor(limit);
		return wholeLimit > 0 ? wholeLimit : null;
	}

	async updateMetadata(id: string, meta: Partial<ChatSessionMeta>): Promise<void> {
		const url = this.getPath(`/${encodeURIComponent(id)}/meta`);
		const res = await fetch(url, {
			method: "POST",
			headers: this.headers,
			body: JSON.stringify(meta),
		});
		await this.assertOk(res, "Failed to update chat metadata", url);
	}

	async delete(id: string): Promise<void> {
		const url = this.getPath(`/${encodeURIComponent(id)}`);
		const res = await fetch(url, {
			method: "DELETE",
			headers: this.headers,
		});
		await this.assertOk(res, "Failed to delete chat", url);
	}

	async loadOlderMessages(
		sessionId: string,
		cursor: string,
		limit: number,
	): Promise<{ messages: Message[]; hasMore: boolean; nextOlderMessagesCursor?: string }> {
		const params = new URLSearchParams({ before: cursor, limit: limit.toString() });
		const url = `${this.getPath(`/${encodeURIComponent(sessionId)}`)}?${params.toString()}`;
		const res = await fetch(url, { headers: this.headers });
		await this.assertOk(res, "Failed to load older messages", url);
		return res.json();
	}

	private async assertOk(res: Response, action: string, url: string): Promise<void> {
		if (res.ok) return;

		let responseBody = "";
		try {
			responseBody = (await res.text()).trim();
		} catch {
			responseBody = "";
		}

		throw new RemoteStorageError(action, res.status, url, responseBody);
	}
}
