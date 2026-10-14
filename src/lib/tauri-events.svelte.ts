// The one way the frontend listens to Tauri events (#362).
//
//   // In a component's script: registered when it mounts, removed when it is destroyed.
//   useTauriEvent("hotkey-registration-changed", (status) => (hotkeyOk = status.ok));
//
//   // For a listener with its own lifetime (one run, one request):
//   const progress = subscribeTauriEvent("plain-transcription-progress", onProgress);
//   try {
//     await progress.ready; // the listener is active before native work starts
//     await run();
//   } finally {
//     progress.dispose();
//   }
//
// A raw `listen()` resolves with its `unlisten` only after an IPC round trip.
// Dropping that promise leaks the listener, and a component destroyed before it
// resolves has nothing left to call. A subscription owns the registration from
// the start: `dispose()` before or after it settles ends in one `unlisten()`,
// and the handler never runs once the subscription is disposed.
import { untrack } from "svelte";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview, type DragDropEvent } from "@tauri-apps/api/webview";
import type { EventMap, EventName } from "./events";

/** Stops one listener (what `listen()` and `onDragDropEvent()` resolve with). */
export type Unlisten = () => void | Promise<void>;

export interface Subscription {
  /**
   * Resolves once the listener is active, and rejects with the registration
   * failure (already logged: awaiting it is for callers that must not go on
   * without the listener). It also resolves when the subscription was disposed
   * first: the late listener was released instead.
   */
  readonly ready: Promise<void>;
  /** Stops the listener and any handler call after it. Safe to call again. */
  dispose(): void;
}

/**
 * Own one listener registration. `register` receives `active()`, which turns
 * false at `dispose()`: wrap the handler in it, so that an event delivered
 * while the registration or the removal is in flight is dropped. Registration
 * starts immediately.
 */
export function subscribe(label: string, register: (active: () => boolean) => Promise<Unlisten>): Subscription {
  let disposed = false;
  let unlisten: Unlisten | null = null;

  const ready = (async () => {
    let registered: Unlisten;
    try {
      registered = await register(() => !disposed);
    } catch (error) {
      console.warn(`Could not listen for ${label}`, error);
      throw error;
    }
    if (disposed) release(label, registered);
    else unlisten = registered;
  })();
  // The warning above is the report. A caller that never awaits `ready` must not
  // turn the failure into an unhandled rejection as well.
  ready.catch(() => undefined);

  return {
    ready,
    dispose() {
      if (disposed) return;
      disposed = true;
      const registered = unlisten;
      unlisten = null;
      if (registered) release(label, registered);
    },
  };
}

function release(label: string, unlisten: Unlisten): void {
  try {
    // Tauri's unlisten is async and rejects when the IPC call does.
    Promise.resolve(unlisten()).catch((error) => console.warn(`Could not stop listening for ${label}`, error));
  } catch (error) {
    console.warn(`Could not stop listening for ${label}`, error);
  }
}

/** Listen to a backend event until `dispose()`. The handler receives the payload. */
export function subscribeTauriEvent<E extends EventName>(name: E, handler: (payload: EventMap[E]) => void): Subscription {
  return subscribe(`"${name}"`, (active) =>
    listen<EventMap[E]>(name, (event) => {
      if (active()) handler(event.payload);
    }),
  );
}

/** Listen to files dragged over and dropped on this window until `dispose()`. */
export function subscribeDragDrop(handler: (payload: DragDropEvent) => void): Subscription {
  return subscribe("drag and drop", (active) =>
    getCurrentWebview().onDragDropEvent((event) => {
      if (active()) handler(event.payload);
    }),
  );
}

/**
 * Run `start` in the component's mount effect and stop it when the effect is
 * cleaned up. Starting at init and stopping in `onDestroy` would leak when the
 * component is destroyed before its first effect flush: `onDestroy` is itself
 * an effect, so its cleanup never exists. `ready` is available at once; it
 * settles once the effect started the subscription and stays pending when the
 * component is destroyed before that.
 */
function forTheLifeOfTheComponent(start: () => Subscription): Subscription {
  let settle!: { resolve: () => void; reject: (reason: unknown) => void };
  const ready = new Promise<void>((resolve, reject) => {
    settle = { resolve, reject };
  });
  ready.catch(() => undefined);
  let current: Subscription | null = null;
  let disposed = false;

  $effect(() => {
    if (disposed) return;
    // Nothing the registration reads is a dependency of this effect.
    const subscription = untrack(start);
    current = subscription;
    subscription.ready.then(settle.resolve, settle.reject);
    return subscription.dispose;
  });

  return {
    ready,
    dispose() {
      disposed = true;
      current?.dispose();
    },
  };
}

/**
 * Listen to a backend event for the life of the component. Call it while the
 * component initializes (its script, not an `onMount` callback); the listener
 * registers in the component's mount effect, before its own `onMount`
 * callbacks, and is removed when the component is destroyed, also when that
 * happens before the registration finished or before the component mounted.
 * Use `ready` to order work after the listener.
 */
export function useTauriEvent<E extends EventName>(name: E, handler: (payload: EventMap[E]) => void): Subscription {
  return forTheLifeOfTheComponent(() => subscribeTauriEvent(name, handler));
}

/** `useTauriEvent` for files dragged over and dropped on the window. */
export function useDragDropEvent(handler: (payload: DragDropEvent) => void): Subscription {
  return forTheLifeOfTheComponent(() => subscribeDragDrop(handler));
}
