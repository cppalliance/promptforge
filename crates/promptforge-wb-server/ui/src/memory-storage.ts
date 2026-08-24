import type {
  ChatSession,
  ChatSessionMeta,
  ChatStorage,
  PaginatedSessions,
} from "./chat/core/types";

/**
 * ChatStorage backed by a page-local Map: sessions work within the page's
 * lifetime and vanish on reload, matching the pre-migration UI whose history
 * was a page-local array. The server-side JSONL tape remains the durable
 * record of every exchange.
 */
export class MemoryStorage implements ChatStorage {
  private sessions = new Map<string, ChatSession>();

  loadSessions(): Promise<PaginatedSessions> {
    const items: ChatSessionMeta[] = [...this.sessions.values()]
      .map((session) => ({
        id: session.id,
        title: session.title,
        updatedAt: session.updatedAt,
      }))
      .sort((a, b) => b.updatedAt - a.updatedAt);
    return Promise.resolve({ items, hasMore: false });
  }

  loadOne(id: string): Promise<ChatSession | null> {
    return Promise.resolve(this.sessions.get(id) ?? null);
  }

  save(session: ChatSession): Promise<void> {
    this.sessions.set(session.id, session);
    return Promise.resolve();
  }

  delete(id: string): Promise<void> {
    this.sessions.delete(id);
    return Promise.resolve();
  }
}
