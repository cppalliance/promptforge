const MAX_EVENT_SIZE = 1024 * 1024;

/**
 * Parses a Server-Sent Events (SSE) stream from a fetch Response.
 * * NOTE: This is a specialized parser tailored for LLM streaming.
 * It intentionally ignores standard SSE fields such as `event:`, `id:`,
 * and `retry:`. It strictly extracts and concatenates `data:` fields.
 *
 * @param response The Response object from `fetch()`
 * @param onMessage Callback fired for every payload.
 * Return `true` from the callback to cancel the stream.
 */
export async function parseSSE(response: Response, onMessage: (data: string) => boolean | undefined): Promise<void> {
	if (!response.body) throw new Error("No response body");

	const reader = response.body.getReader();
	const decoder = new TextDecoder("utf-8");
	let buffer = "";

	try {
		while (true) {
			const { done, value } = await reader.read();

			if (value) {
				buffer += decoder.decode(value, { stream: true });
			}

			if (done) {
				buffer += decoder.decode();
			}

			if (buffer.length > MAX_EVENT_SIZE) {
				throw new Error("SSE parse error: event buffer exceeded 1MB limit.");
			}

			while (true) {
				const nIdx = buffer.indexOf("\n\n");
				const rIdx = buffer.indexOf("\r\n\r\n");

				let boundaryIdx = -1;
				let skipChars = 0;

				if (nIdx !== -1 && (rIdx === -1 || nIdx < rIdx)) {
					boundaryIdx = nIdx;
					skipChars = 2;
				} else if (rIdx !== -1) {
					boundaryIdx = rIdx;
					skipChars = 4;
				}

				if (boundaryIdx === -1) break;

				const eventStr = buffer.substring(0, boundaryIdx);
				buffer = buffer.substring(boundaryIdx + skipChars);

				if (eventStr.length > 0) {
					const data = parseEventData(eventStr);
					// Strictly check against null; empty string is a valid event payload.
					if (data !== null) {
						if (onMessage(data)) {
							await reader.cancel();
							return;
						}
					}
				}
			}

			if (done) break;
		}

		if (buffer.length > 0) {
			const data = parseEventData(buffer);
			if (data !== null) onMessage(data);
		}
	} catch (error) {
		// Tear down the connection on the error path too; releaseLock() alone
		// leaves the HTTP response streaming until the server closes it.
		try {
			await reader.cancel();
		} catch {
			// Surfacing the original error matters more.
		}
		throw error;
	} finally {
		reader.releaseLock();
	}
}

function parseEventData(eventStr: string): string | null {
	let data: string | null = null;
	let start = 0;

	while (start < eventStr.length) {
		let end = eventStr.indexOf("\n", start);
		if (end === -1) end = eventStr.length;

		let line = eventStr.substring(start, end);

		// Handle \r\n endings safely
		if (line.endsWith("\r")) {
			line = line.substring(0, line.length - 1);
		}

		if (line.startsWith("data:")) {
			let val = line.substring(5);
			// The SSE standard dictates stripping exactly ONE leading space if present.
			if (val.startsWith(" ")) {
				val = val.substring(1);
			}

			if (data === null) {
				data = val;
			} else {
				data += "\n" + val;
			}
		}

		start = end + 1;
	}

	return data;
}
