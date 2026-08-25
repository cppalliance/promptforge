/**
 * Generates a UUIDv7 (Time-ordered).
 */
export function uuidv7(): string {
	// 1. 48-bit timestamp in milliseconds (12 hex chars)
	const timeHex = Date.now().toString(16).padStart(12, "0");

	// 2. We need 10 more random bytes (80 bits)
	const bytes = new Uint8Array(10);
	crypto.getRandomValues(bytes);

	// 3. Byte 0: Version indicator (4 bits, value 7) + 4 random bits
	const g3 = (0x70 | (bytes[0] & 0x0f)).toString(16).padStart(2, "0") + bytes[1].toString(16).padStart(2, "0");

	// 4. Byte 2: Variant indicator (2 bits, value 10 binary) + 6 random bits
	const g4 = (0x80 | (bytes[2] & 0x3f)).toString(16).padStart(2, "0") + bytes[3].toString(16).padStart(2, "0");

	// 5. Bytes 4-9: 6 bytes of pure randomness (12 hex chars)
	let g5 = "";
	for (let i = 4; i < 10; i++) {
		g5 += bytes[i].toString(16).padStart(2, "0");
	}

	// 6. Format: 8-4-4-4-12
	return `${timeHex.substring(0, 8)}-${timeHex.substring(8)}-${g3}-${g4}-${g5}`;
}
