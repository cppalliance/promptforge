export type RouterType = "hash" | "path" | "none";

export interface RouterConfig {
	type?: RouterType;
	pathPrefix?: string; // Default: '/c/' for path, '#/chat/' for hash
}

export class AppRouter {
	private type: RouterType;
	private prefix: string;
	private handleNavigate?: () => void;

	constructor(config?: RouterConfig) {
		this.type = config?.type || "hash";

		if (this.type === "path") {
			this.prefix = config?.pathPrefix || "/c/";
		} else {
			this.prefix = config?.pathPrefix || "#/chat/";
		}
	}

	public getId(): string | null {
		if (this.type === "none") return null;

		if (this.type === "path") {
			const path = window.location.pathname;
			if (path.startsWith(this.prefix)) {
				return this.decodeId(path.slice(this.prefix.length));
			}
		} else if (this.type === "hash") {
			const hash = window.location.hash;
			if (hash.startsWith(this.prefix)) {
				return this.decodeId(hash.slice(this.prefix.length));
			}
		}
		return null;
	}

	public hrefFor(id: string): string {
		if (this.type === "none") return "#";
		return `${this.prefix}${encodeURIComponent(id)}`;
	}

	public setUrl(id: string | null, replace = false) {
		if (this.type === "none") return;

		const currentId = this.getId();
		if (currentId === id) return;

		const newUrl = id ? this.hrefFor(id) : this.emptyUrl();

		if (replace) {
			history.replaceState(null, "", newUrl);
		} else {
			history.pushState(null, "", newUrl);
		}
	}

	public listen(onNavigate: (id: string | null) => void) {
		if (this.type === "none") return;

		this.handleNavigate = () => {
			onNavigate(this.getId());
		};

		for (const eventType of this.eventTypes()) {
			window.addEventListener(eventType, this.handleNavigate);
		}
	}

	public destroy() {
		if (this.type === "none" || !this.handleNavigate) return;
		for (const eventType of this.eventTypes()) {
			window.removeEventListener(eventType, this.handleNavigate);
		}
		this.handleNavigate = undefined;
	}

	private eventTypes(): ("hashchange" | "popstate")[] {
		return this.type === "hash" ? ["hashchange", "popstate"] : ["popstate"];
	}

	private decodeId(value: string): string | null {
		try {
			return decodeURIComponent(value);
		} catch {
			return null;
		}
	}

	private emptyUrl(): string {
		if (this.type === "hash") return this.emptyHashUrl();
		return this.emptyPathUrl();
	}

	private emptyPathUrl(): string {
		const trimmed = this.prefix.endsWith("/") ? this.prefix.slice(0, -1) : this.prefix;
		const slashIndex = trimmed.lastIndexOf("/");
		if (slashIndex <= 0) return "/";
		return `${trimmed.slice(0, slashIndex)}/`;
	}

	private emptyHashUrl(): string {
		const hashPath = this.prefix.startsWith("#") ? this.prefix.slice(1) : this.prefix;
		const trimmed = hashPath.endsWith("/") ? hashPath.slice(0, -1) : hashPath;
		const slashIndex = trimmed.lastIndexOf("/");
		if (slashIndex <= 0) return "#/";
		return `#${trimmed.slice(0, slashIndex)}/`;
	}
}
