// The recorder's two events, listened to through useTauriEvent (SAGA-564).
//
// TODO(SAGA-564): `EventMap` (src/lib/events.ts) does not list these events
// yet: adding them before the Rust catalogue and contracts/transport/events.json
// have them fails src/test/transport.test.ts. Once they do, add the two entries
// to `EventMap` and `eventPayloads`, delete this file's cast and call
// `useTauriEvent` directly.
import type { EventMap, EventName } from "./events";
import type { RecordingSessionInfo, RecoveryNote } from "./record-types";
import { useTauriEvent, type Subscription } from "./tauri-events.svelte";

export interface RecorderEventMap {
  /** On every state change and every 250 ms while a session records or finalizes. */
  "recording-session-changed": RecordingSessionInfo;
  /** Sent once after app start when recovery kept anything. */
  "recordings-recovered": RecoveryNote[];
}

export type RecorderEventName = keyof RecorderEventMap;

/** Listen to a recorder event for the life of the component (call it while the component initializes). */
export function useRecorderEvent<E extends RecorderEventName>(name: E, handler: (payload: RecorderEventMap[E]) => void): Subscription {
  return useTauriEvent(name as unknown as EventName, handler as unknown as (payload: EventMap[EventName]) => void);
}
