// The listener helper (#362): ownership of a registration from the first
// call to dispose, and its wiring to Tauri events and to a component's life.
import { emit } from "@tauri-apps/api/event";
import { mockWindows } from "@tauri-apps/api/mocks";
import { render, waitFor } from "@testing-library/svelte";
import { describe, expect, test, vi } from "vitest";
import { subscribe, subscribeDragDrop, subscribeTauriEvent, type Subscription, type Unlisten } from "./tauri-events.svelte";
import EventProbe from "../test/EventProbe.svelte";
import { deferred, settle } from "../test/deferred";
import { mockIpc } from "../test/ipc";

function quietWarnings() {
  return vi.spyOn(console, "warn").mockImplementation(() => undefined);
}

describe("subscribe: owning one registration", () => {
  test("registration starts at once, before anything awaits it", () => {
    const calls: string[] = [];
    subscribe("a", async () => {
      calls.push("a");
      return () => undefined;
    });
    subscribe("b", async () => {
      calls.push("b");
      return () => undefined;
    });
    calls.push("caller continues");

    expect(calls).toEqual(["a", "b", "caller continues"]);
  });

  test("dispose stops the listener once, however often it is called", async () => {
    const unlisten = vi.fn();
    const subscription = subscribe("a", async () => unlisten);
    await subscription.ready;
    expect(unlisten).not.toHaveBeenCalled();

    subscription.dispose();
    subscription.dispose();

    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  test("a registration that settles after dispose is released the moment it settles", async () => {
    const registration = deferred<Unlisten>();
    const unlisten = vi.fn();
    const subscription = subscribe("a", () => registration.promise);

    subscription.dispose();
    expect(unlisten).not.toHaveBeenCalled();
    registration.resolve(unlisten);

    await subscription.ready;
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  test("the handler's gate closes at dispose, while the registration is still in flight", async () => {
    const registration = deferred<Unlisten>();
    let active: () => boolean = () => false;
    const subscription = subscribe("a", (isActive) => {
      active = isActive;
      return registration.promise;
    });
    expect(active()).toBe(true);

    subscription.dispose();

    expect(active()).toBe(false);
    registration.resolve(() => undefined);
    await subscription.ready;
  });

  test("a failed registration is reported once and rejects ready; nothing is left to stop", async () => {
    const warn = quietWarnings();
    const failure = new Error("ipc down");
    const subscription = subscribe("state-changed", async () => {
      throw failure;
    });

    await expect(subscription.ready).rejects.toBe(failure);
    expect(warn).toHaveBeenCalledWith("Could not listen for state-changed", failure);
    expect(() => subscription.dispose()).not.toThrow();
  });

  test("when one of several registrations fails, disposing the others still releases them", async () => {
    quietWarnings();
    const unlisten = vi.fn();
    const progress = subscribe("progress", async () => unlisten);
    const ready = subscribe("ready", async () => {
      throw new Error("ipc down");
    });
    await progress.ready;
    await expect(ready.ready).rejects.toThrow("ipc down");

    progress.dispose();
    ready.dispose();

    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  test("a failure nobody awaits is only a warning, not an unhandled rejection", async () => {
    const warn = quietWarnings();
    subscribe("a", () => Promise.reject(new Error("ipc down")));

    await settle(10);

    expect(warn).toHaveBeenCalledTimes(1);
  });

  test("a registration that throws before returning a promise is a failed registration", async () => {
    const warn = quietWarnings();
    const failure = new Error("no window");
    const subscription = subscribe("drag and drop", () => {
      throw failure;
    });

    await expect(subscription.ready).rejects.toBe(failure);
    expect(warn).toHaveBeenCalledWith("Could not listen for drag and drop", failure);
  });

  test("an unlisten that fails is reported, never thrown at the caller or left unhandled", async () => {
    const warn = quietWarnings();
    const async = subscribe("a", async () => () => Promise.reject(new Error("already gone")));
    const sync = subscribe("b", async () => () => {
      throw new Error("threw");
    });
    await Promise.all([async.ready, sync.ready]);

    expect(() => {
      async.dispose();
      sync.dispose();
    }).not.toThrow();
    await settle(10);

    expect(warn.mock.calls.map(([message]) => message)).toEqual(["Could not stop listening for b", "Could not stop listening for a"]);
  });
});

describe("subscribeTauriEvent", () => {
  test("hands the payload to the handler until disposed", async () => {
    const ipc = mockIpc();
    const received: string[] = [];
    const subscription = subscribeTauriEvent("state-changed", (state) => received.push(state));
    await subscription.ready;

    ipc.emit("state-changed", "recording");
    ipc.emit("state-changed", "idle");
    subscription.dispose();
    await waitFor(() => expect(ipc.listeners()).toEqual([]));

    expect(received).toEqual(["recording", "idle"]);
  });

  test("an event that reaches a listener disposed while still registering is dropped", async () => {
    const ipc = mockIpc();
    const handler = vi.fn();
    const subscription = subscribeTauriEvent("state-changed", handler);

    subscription.dispose();
    // The backend registered it already; the unlisten() waits for its reply.
    expect(ipc.emit("state-changed", "recording")).toBe(1);

    expect(handler).not.toHaveBeenCalled();
    await subscription.ready;
    await waitFor(() => expect(ipc.listeners()).toEqual([]));
  });

  test("disposed before the backend confirmed the listener, it is removed once confirmed", async () => {
    const ipc = mockIpc();
    const subscription = subscribeTauriEvent("model-ready", vi.fn());

    subscription.dispose();
    await subscription.ready;

    await waitFor(() => expect(ipc.listeners()).toEqual([]));
  });

  test("two subscriptions to one event are independent", async () => {
    const ipc = mockIpc();
    const first = vi.fn();
    const second = vi.fn();
    const a = subscribeTauriEvent("state-changed", first);
    const b = subscribeTauriEvent("state-changed", second);
    await Promise.all([a.ready, b.ready]);

    a.dispose();
    await waitFor(() => expect(ipc.listenerCount("state-changed")).toBe(1));
    ipc.emit("state-changed", "idle");

    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledWith("idle");
    b.dispose();
  });
});

describe("subscribeDragDrop", () => {
  test("hands over the drag and drop events of this window, and removes all four listeners", async () => {
    const ipc = mockIpc();
    mockWindows("main");
    const received: unknown[] = [];
    const subscription = subscribeDragDrop((payload) => received.push(payload));
    await subscription.ready;
    expect(ipc.listeners()).toEqual(["tauri://drag-drop", "tauri://drag-enter", "tauri://drag-leave", "tauri://drag-over"]);

    await emit("tauri://drag-over", { position: { x: 1, y: 2 } });
    await emit("tauri://drag-drop", { paths: ["/audio/a.wav"], position: { x: 3, y: 4 } });
    subscription.dispose();
    await waitFor(() => expect(ipc.listeners()).toEqual([]));

    expect(received).toMatchObject([{ type: "over" }, { type: "drop", paths: ["/audio/a.wav"] }]);
  });

  test("outside a Tauri window the registration fails visibly, and the caller carries on", async () => {
    mockIpc();
    const warn = quietWarnings();
    const subscription = subscribeDragDrop(vi.fn());

    await expect(subscription.ready).rejects.toThrow();
    expect(warn).toHaveBeenCalledWith("Could not listen for drag and drop", expect.anything());
  });
});

describe("listening for the life of a component", () => {
  test("a component's listeners start with it and end when it is destroyed", async () => {
    const ipc = mockIpc();
    mockWindows("main");
    const states: string[] = [];
    let subscriptions: Subscription[] = [];
    const view = render(EventProbe, {
      onState: (state: string) => states.push(state),
      onDrop: () => undefined,
      onSubscribed: (created: Subscription[]) => (subscriptions = created),
    });
    await Promise.all(subscriptions.map((subscription) => subscription.ready));
    expect(ipc.listeners()).toEqual([
      "state-changed",
      "tauri://drag-drop",
      "tauri://drag-enter",
      "tauri://drag-leave",
      "tauri://drag-over",
    ]);

    ipc.emit("state-changed", "recording");
    view.unmount();

    await waitFor(() => expect(ipc.listeners()).toEqual([]));
    expect(states).toEqual(["recording"]);
  });

  test("destroyed before the registration finished, it still ends with no listener", async () => {
    const ipc = mockIpc();
    const handler = vi.fn();
    let subscriptions: Subscription[] = [];
    render(EventProbe, { onState: handler, onSubscribed: (created: Subscription[]) => (subscriptions = created) }).unmount();
    await Promise.all(subscriptions.map((subscription) => subscription.ready));

    await waitFor(() => expect(ipc.listeners()).toEqual([]));
    expect(handler).not.toHaveBeenCalled();
  });
});
