export class Store<T extends object> {
	private state: T;
	private selectorListeners: Set<(state: T) => void> = new Set();
	private hotListeners: Set<(state: T) => void> = new Set();

	constructor(initialState: T) {
		this.state = initialState;
	}

	get(): T {
		return this.state;
	}

	/**
	 * Standard immutable update.
	 * Use this for 99% of state changes (sessions, active chat, etc).
	 * Safely triggers all relevant selector-based subscribers.
	 */
	set(partialState: Partial<T>) {
		this.state = { ...this.state, ...partialState };
		this.notifySelectorListeners();
		this.notifyHotListeners();
	}

	/**
	 * HIGH-PERFORMANCE HOT PATH ONLY.
	 * Mutates state in-place to prevent GC thrashing during LLM streaming.
	 * NOTE: This intentionally bypasses selector subscribers so hot updates
	 * do not run every selector on every token. Only hot subscribers are notified.
	 * Hot subscribers receive the live mutable state object; they must not retain
	 * references to state or nested slices across notifications.
	 */
	mutateHot(recipe: (state: T) => void) {
		recipe(this.state);
		this.notifyHotListeners();
	}

	/**
	 * Subscribes to a specific slice of state.
	 * The listener fires IMMEDIATELY with the current state, and then
	 * whenever the selected value actually changes.
	 */
	subscribe<U>(selector: (state: T) => U, listener: (selectedState: U) => void): () => void {
		const initialSlice = selector(this.state);
		listener(initialSlice);
		return this.onChangeFrom(selector, listener, initialSlice);
	}

	/**
	 * Subscribes to normal set() updates and hot in-place mutations.
	 * Fires IMMEDIATELY with the current state, then on subsequent updates.
	 * Use sparingly for render paths that must observe high-frequency mutable state.
	 * The listener receives the live mutable state object; do not retain references
	 * to state or nested slices because mutateHot may change them in-place.
	 */
	subscribeHot(listener: (state: T) => void): () => void {
		listener(this.state);
		this.hotListeners.add(listener);
		return () => this.hotListeners.delete(listener);
	}
	/**
	 * Subscribes to a specific slice of state.
	 * The listener ONLY fires on future changes, not immediately.
	 */
	public onChange<U>(selector: (state: T) => U, listener: (selectedState: U) => void): () => void {
		return this.onChangeFrom(selector, listener, selector(this.state));
	}

	private onChangeFrom<U>(
		selector: (state: T) => U,
		listener: (selectedState: U) => void,
		initialSlice: U,
	): () => void {
		let lastSlice = initialSlice;
		const wrappedListener = (state: T) => {
			const currentSlice = selector(state);
			if (currentSlice !== lastSlice) {
				lastSlice = currentSlice;
				listener(currentSlice);
			}
		};
		this.selectorListeners.add(wrappedListener);
		return () => this.selectorListeners.delete(wrappedListener);
	}

	public clearAllListeners(): void {
		this.selectorListeners.clear();
		this.hotListeners.clear();
	}

	private notifySelectorListeners() {
		for (const listener of this.selectorListeners) {
			listener(this.state);
		}
	}

	private notifyHotListeners() {
		for (const listener of this.hotListeners) {
			listener(this.state);
		}
	}
}
