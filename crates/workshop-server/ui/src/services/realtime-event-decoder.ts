const HYPOTHESIS_INCLUDE = "item.input_audio_transcription.hypothesis";

interface RealtimeAudioFormat {
  readonly type: "audio/pcm";
  readonly rate: 24000;
}

interface RealtimeTranscriptionConfiguration {
  readonly model: "realtime-transcribe";
  readonly prompt: string;
}

interface RealtimeEffectiveSession {
  readonly id: string;
  readonly object: "realtime.transcription_session";
  readonly type: "transcription";
  readonly audio: {
    readonly input: {
      readonly format: RealtimeAudioFormat;
      readonly noise_reduction: null;
      readonly transcription: RealtimeTranscriptionConfiguration;
      readonly turn_detection: null;
    };
  };
  readonly include: readonly [] | readonly [typeof HYPOTHESIS_INCLUDE];
}

interface RealtimeWireError {
  readonly type: string;
  readonly code: string;
  readonly message: string;
  readonly param?: string | null;
  readonly event_id?: string | null;
}

interface RealtimeConversationItem {
  readonly id: string;
  readonly type: "message";
  readonly status: "completed";
  readonly role: "user";
  readonly content: readonly [
    {
      readonly type: "input_audio";
      readonly transcript: null;
    },
  ];
}

interface RealtimeDurationUsage {
  readonly type: "duration";
  readonly seconds: number;
}

/** A fully validated server event from the Realtime transcription wire. */
export type RealtimeEvent =
  | {
      readonly type: "session.created";
      readonly event_id: string;
      readonly session: RealtimeEffectiveSession;
    }
  | {
      readonly type: "session.updated";
      readonly event_id: string;
      readonly session: RealtimeEffectiveSession;
    }
  | {
      readonly type: "input_audio_buffer.committed";
      readonly event_id: string;
      readonly item_id: string;
      readonly previous_item_id: string | null;
    }
  | {
      readonly type: "input_audio_buffer.cleared";
      readonly event_id: string;
    }
  | {
      readonly type: "conversation.item.created";
      readonly event_id: string;
      readonly previous_item_id: string | null;
      readonly item: RealtimeConversationItem;
    }
  | {
      readonly type: "conversation.item.input_audio_transcription.delta";
      readonly event_id: string;
      readonly item_id: string;
      readonly content_index: 0;
      readonly delta: string;
    }
  | {
      readonly type: "conversation.item.input_audio_transcription.completed";
      readonly event_id: string;
      readonly item_id: string;
      readonly content_index: 0;
      readonly transcript: string;
      readonly usage: RealtimeDurationUsage;
    }
  | {
      readonly type: "conversation.item.input_audio_transcription.failed";
      readonly event_id: string;
      readonly item_id: string;
      readonly content_index: 0;
      readonly error: Omit<RealtimeWireError, "event_id">;
    }
  | {
      readonly type: "conversation.item.input_audio_transcription.hypothesis";
      readonly event_id: string;
      readonly item_id: string;
      readonly content_index: 0;
      readonly revision: number;
      readonly transcript: string;
      readonly finalized: string;
      readonly agreed: string;
      readonly tentative: string;
      readonly audio_start_ms: number;
      readonly audio_end_ms: number;
    }
  | {
      readonly type: "error";
      readonly event_id: string;
      readonly error: RealtimeWireError;
    };

function exactRecord(
  value: unknown,
  required: readonly string[],
  optional: readonly string[] = [],
): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const allowed = new Set([...required, ...optional]);
  const keys = Reflect.ownKeys(value);
  return (
    required.every((field) => Object.hasOwn(value, field)) &&
    keys.every((field) => typeof field === "string" && allowed.has(field))
  );
}

function nonemptyString(value: unknown): value is string {
  return typeof value === "string" && value.length > 0;
}

function nullableId(value: unknown): value is string | null {
  return value === null || nonemptyString(value);
}

function unsignedSafeInteger(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function effectiveSession(value: unknown): value is RealtimeEffectiveSession {
  if (
    !exactRecord(value, ["id", "object", "type", "audio", "include"]) ||
    !nonemptyString(value.id) ||
    value.object !== "realtime.transcription_session" ||
    value.type !== "transcription" ||
    !Array.isArray(value.include) ||
    !(
      value.include.length === 0 ||
      (value.include.length === 1 && value.include[0] === HYPOTHESIS_INCLUDE)
    ) ||
    !exactRecord(value.audio, ["input"])
  ) {
    return false;
  }
  const input = value.audio.input;
  if (
    !exactRecord(input, [
      "format",
      "noise_reduction",
      "transcription",
      "turn_detection",
    ]) ||
    input.noise_reduction !== null ||
    input.turn_detection !== null ||
    !exactRecord(input.format, ["type", "rate"]) ||
    input.format.type !== "audio/pcm" ||
    input.format.rate !== 24000 ||
    !exactRecord(input.transcription, ["model", "prompt"]) ||
    input.transcription.model !== "realtime-transcribe" ||
    typeof input.transcription.prompt !== "string"
  ) {
    return false;
  }
  return true;
}

function wireError(value: unknown, allowEventId: boolean): value is RealtimeWireError {
  const optional = allowEventId ? ["param", "event_id"] : ["param"];
  if (
    !exactRecord(value, ["type", "code", "message"], optional) ||
    !nonemptyString(value.type) ||
    !nonemptyString(value.code) ||
    !nonemptyString(value.message)
  ) {
    return false;
  }
  if (Object.hasOwn(value, "param") && !nullableId(value.param)) {
    return false;
  }
  return !Object.hasOwn(value, "event_id") || nullableId(value.event_id);
}

function conversationItem(value: unknown): value is RealtimeConversationItem {
  if (
    !exactRecord(value, ["id", "type", "status", "role", "content"]) ||
    !nonemptyString(value.id) ||
    value.type !== "message" ||
    value.status !== "completed" ||
    value.role !== "user" ||
    !Array.isArray(value.content) ||
    value.content.length !== 1
  ) {
    return false;
  }
  const content = value.content[0];
  return (
    exactRecord(content, ["type", "transcript"]) &&
    content.type === "input_audio" &&
    content.transcript === null
  );
}

function durationUsage(value: unknown): value is RealtimeDurationUsage {
  return (
    exactRecord(value, ["type", "seconds"]) &&
    value.type === "duration" &&
    typeof value.seconds === "number" &&
    Number.isFinite(value.seconds) &&
    value.seconds >= 0
  );
}

function transcriptionBase(
  value: Record<string, unknown>,
  fields: readonly string[],
): boolean {
  return (
    exactRecord(value, fields) &&
    nonemptyString(value.event_id) &&
    nonemptyString(value.item_id) &&
    value.content_index === 0
  );
}

/**
 * Validates an unknown Realtime server value without side effects.
 * Unsupported types and malformed event shapes return null.
 */
export function decodeRealtimeEvent(value: unknown): RealtimeEvent | null {
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    typeof (value as Record<string, unknown>).type !== "string"
  ) {
    return null;
  }
  const event = value as Record<string, unknown>;
  switch (event.type) {
    case "session.created":
    case "session.updated":
      if (
        exactRecord(event, ["event_id", "type", "session"]) &&
        nonemptyString(event.event_id) &&
        effectiveSession(event.session)
      ) {
        return event as RealtimeEvent;
      }
      return null;
    case "input_audio_buffer.committed":
      if (
        exactRecord(event, [
          "event_id",
          "type",
          "item_id",
          "previous_item_id",
        ]) &&
        nonemptyString(event.event_id) &&
        nonemptyString(event.item_id) &&
        nullableId(event.previous_item_id)
      ) {
        return event as RealtimeEvent;
      }
      return null;
    case "input_audio_buffer.cleared":
      if (
        exactRecord(event, ["event_id", "type"]) &&
        nonemptyString(event.event_id)
      ) {
        return event as RealtimeEvent;
      }
      return null;
    case "conversation.item.created":
      if (
        exactRecord(event, [
          "event_id",
          "type",
          "previous_item_id",
          "item",
        ]) &&
        nonemptyString(event.event_id) &&
        nullableId(event.previous_item_id) &&
        conversationItem(event.item)
      ) {
        return event as RealtimeEvent;
      }
      return null;
    case "conversation.item.input_audio_transcription.delta":
      if (
        transcriptionBase(event, [
          "event_id",
          "type",
          "item_id",
          "content_index",
          "delta",
        ]) &&
        typeof event.delta === "string"
      ) {
        return event as RealtimeEvent;
      }
      return null;
    case "conversation.item.input_audio_transcription.completed":
      if (
        transcriptionBase(event, [
          "event_id",
          "type",
          "item_id",
          "content_index",
          "transcript",
          "usage",
        ]) &&
        typeof event.transcript === "string" &&
        durationUsage(event.usage)
      ) {
        return event as RealtimeEvent;
      }
      return null;
    case "conversation.item.input_audio_transcription.failed":
      if (
        transcriptionBase(event, [
          "event_id",
          "type",
          "item_id",
          "content_index",
          "error",
        ]) &&
        wireError(event.error, false)
      ) {
        return event as RealtimeEvent;
      }
      return null;
    case "conversation.item.input_audio_transcription.hypothesis":
      if (
        transcriptionBase(event, [
          "event_id",
          "type",
          "item_id",
          "content_index",
          "revision",
          "transcript",
          "finalized",
          "agreed",
          "tentative",
          "audio_start_ms",
          "audio_end_ms",
        ]) &&
        unsignedSafeInteger(event.revision) &&
        typeof event.transcript === "string" &&
        typeof event.finalized === "string" &&
        typeof event.agreed === "string" &&
        typeof event.tentative === "string" &&
        event.transcript === `${event.finalized}${event.agreed}${event.tentative}` &&
        unsignedSafeInteger(event.audio_start_ms) &&
        unsignedSafeInteger(event.audio_end_ms) &&
        event.audio_start_ms <= event.audio_end_ms
      ) {
        return event as RealtimeEvent;
      }
      return null;
    case "error":
      if (
        exactRecord(event, ["event_id", "type", "error"]) &&
        nonemptyString(event.event_id) &&
        wireError(event.error, true)
      ) {
        return event as RealtimeEvent;
      }
      return null;
    default:
      return null;
  }
}
