import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import LiveMeetingPanel, { type LiveApi } from "./LiveMeetingPanel.svelte";
import type { LiveSnapshot } from "./live-meeting-types";
import { deferred } from "../test/deferred";

function snapshot(overrides: Partial<LiveSnapshot> = {}): LiveSnapshot {
  return {
    id: "session-1",
    revision: 1,
    phase: "running",
    elapsed_ms: 1_000,
    sources: [],
    segments: [],
    gaps: [],
    error: null,
    audio_available: true,
    ...overrides,
  };
}

function api(overrides: Partial<LiveApi> = {}): LiveApi {
  return {
    load: vi.fn(async () => null),
    start: vi.fn(async () => undefined),
    stop: vi.fn(async () => undefined),
    discard: vi.fn(async () => undefined),
    saveText: vi.fn(async () => undefined),
    saveAudio: vi.fn(async () => undefined),
    ...overrides,
  };
}

afterEach(() => {
  vi.useRealTimers();
});

test("initial load disables start and stale load results are ignored after unmount", async () => {
  const pending = deferred<LiveSnapshot | null>();
  const fakeApi = api({ load: vi.fn(() => pending.promise) });
  const view = render(LiveMeetingPanel, { api: fakeApi, supported: true });

  expect(screen.getByRole("button", { name: "Start live meeting" })).toBeDisabled();
  view.unmount();
  pending.resolve(snapshot());
  await Promise.resolve();
  expect(screen.queryByRole("heading", { name: "Live meeting" })).not.toBeInTheDocument();
});

test("polling reloads an existing running session without overlapping loads", async () => {
  vi.useFakeTimers();
  const first = deferred<LiveSnapshot | null>();
  const second = deferred<LiveSnapshot | null>();
  const load = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise).mockResolvedValue(null);
  const fakeApi = api({ load });
  render(LiveMeetingPanel, { api: fakeApi, supported: true });

  expect(load).toHaveBeenCalledOnce();
  await vi.advanceTimersByTimeAsync(500);
  expect(load).toHaveBeenCalledOnce();
  first.resolve(snapshot());
  await vi.advanceTimersByTimeAsync(500);
  expect(load).toHaveBeenCalledTimes(2);
  second.resolve(snapshot({ revision: 2 }));
  await vi.runOnlyPendingTimersAsync();
});

test("start and stop reconcile through load, while action requests serialize", async () => {
  const loads = [snapshot({ phase: "running" }), snapshot({ phase: "completed" })];
  const fakeApi = api({
    load: vi.fn(async () => loads.shift() ?? null),
  });
  render(LiveMeetingPanel, { api: fakeApi, supported: true });
  await waitFor(() => expect(screen.getByRole("button", { name: "Stop live meeting" })).toBeInTheDocument());

  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Stop live meeting" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Save text" })).toBeInTheDocument());
  expect(fakeApi.stop).toHaveBeenCalledWith("session-1");
  expect(fakeApi.load).toHaveBeenCalledTimes(2);
});

test("failed save preserves the session and can be retried", async () => {
  let fail = true;
  const saveText = vi.fn(async () => {
    if (fail) {
      fail = false;
      throw { code: "io_error", message: "save failed", retryable: true };
    }
  });
  const fakeApi = api({ load: vi.fn(async () => snapshot({ phase: "completed" })), saveText });
  render(LiveMeetingPanel, { api: fakeApi, supported: true });
  await waitFor(() => expect(screen.getByRole("button", { name: "Save text" })).toBeEnabled());

  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Save text" }));
  await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("save failed"));
  expect(screen.getByRole("button", { name: "Save text" })).toBeEnabled();
  await user.click(screen.getByRole("button", { name: "Save text" }));
  await waitFor(() => expect(saveText).toHaveBeenCalledTimes(2));
  expect(screen.getByRole("button", { name: "Discard" })).toBeInTheDocument();
});
