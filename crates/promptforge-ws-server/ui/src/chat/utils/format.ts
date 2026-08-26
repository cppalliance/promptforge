export const MINUTE_MS = 60_000;
export const HOUR_MS = 3_600_000;

export function formatRelativeTime(elapsedMs: number): string {
	const elapsed = Math.max(0, elapsedMs);
	if (elapsed < MINUTE_MS) return "just now";
	const minutes = Math.floor(elapsed / MINUTE_MS);
	if (minutes < 60) return `${minutes}m ago`;
	const hours = Math.floor(minutes / 60);
	if (hours < 24) return `${hours}h ago`;
	return `${Math.floor(hours / 24)}d ago`;
}

export function formatDuration(durationMs: number): string {
	const safeDurationMs = Math.max(0, durationMs);
	if (safeDurationMs < 1000) return `${Math.round(safeDurationMs)}ms`;

	const totalSeconds = Math.round(safeDurationMs / 1000);
	if (totalSeconds < 60) return `${totalSeconds}s`;

	const minutes = Math.floor(totalSeconds / 60);
	const seconds = totalSeconds % 60;
	return `${minutes}m ${String(seconds).padStart(2, "0")}s`;
}
