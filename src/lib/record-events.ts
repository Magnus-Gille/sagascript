// The recorder's two events, listened to through useTauriEvent (SAGA-564).
// Their payloads are in `EventMap` (src/lib/events.ts), pinned by the Rust
// catalogue and contracts/transport/events.json.
import type { EventMap } from "./events";
import { useTauriEvent, type Subscription } from "./tauri-events.svelte";

export type RecorderEventName = "recording-session-changed" | "recordings-recovered";

/** The recorder's events and their payloads. */
export type RecorderEventMap = Pick<EventMap, RecorderEventName>;

/** Listen to a recorder event for the life of the component (call it while the component initializes). */
export function useRecorderEvent<E extends RecorderEventName>(name: E, handler: (payload: EventMap[E]) => void): Subscription {
  return useTauriEvent(name, handler);
}
