import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import type { ComponentProps } from "svelte";
import LiveMeeting from "./LiveMeeting.svelte";
import type { LiveSnapshot } from "./live-meeting-types";

function snapshot(overrides: Partial<LiveSnapshot> = {}): LiveSnapshot {
  return {
    id: "session-1",
    revision: 1,
    phase: "running",
    elapsed_ms: 2_000,
    sources: [
      {
        source: "microphone",
        connected: true,
        signal_level: 0.5,
        last_audio_ms: 1_900,
        last_text_ms: 1_700,
        text_delay_ms: null,
        queued_ms: 0,
        dropped_frames: 0,
        error: null,
      },
      {
        source: "system",
        connected: true,
        signal_level: 0.2,
        last_audio_ms: null,
        last_text_ms: null,
        text_delay_ms: null,
        queued_ms: 0,
        dropped_frames: 0,
        error: null,
      },
    ],
    segments: [],
    gaps: [],
    error: null,
    audio_available: true,
    ...overrides,
  };
}

function renderLive(overrides: Partial<ComponentProps<typeof LiveMeeting>> = {}) {
  const callbacks = {
    onStart: vi.fn(async (): Promise<void> => undefined),
    onStop: vi.fn(async (): Promise<void> => undefined),
    onDiscard: vi.fn(async (): Promise<void> => undefined),
    onSaveText: vi.fn(async (): Promise<void> => undefined),
    onSaveAudio: vi.fn(async (): Promise<void> => undefined),
    onRefresh: vi.fn(async (): Promise<void> => undefined),
  };
  const view = render(LiveMeeting, {
    snapshot: null,
    busy: false,
    supported: true,
    error: null,
    ...callbacks,
    ...overrides,
  });
  return { ...callbacks, ...view };
}

test("starts without rendering an always-on overlay", async () => {
  const callbacks = renderLive();
  await userEvent.setup().click(screen.getByRole("button", { name: "Start live meeting" }));
  expect(callbacks.onStart).toHaveBeenCalledOnce();
  expect(screen.queryByRole("log")).not.toBeInTheDocument();
});

test("running sessions can be stopped and show per-channel health", () => {
  renderLive({ snapshot: snapshot() });
  expect(screen.getByRole("button", { name: "Stop live meeting" })).toBeEnabled();
  expect(screen.getByLabelText("Channel health")).toHaveTextContent("Jag · mikrofon");
  expect(screen.getByLabelText("Channel health")).toHaveTextContent("Mötet · systemljud");
  expect(screen.getByLabelText("Channel health")).toHaveTextContent("Silent");
});

test("starting sessions offer an explicit cancel action", async () => {
  const callbacks = renderLive({ snapshot: snapshot({ phase: "starting" }) });
  await userEvent.setup().click(screen.getByRole("button", { name: "Cancel starting" }));
  expect(callbacks.onStop).toHaveBeenCalledOnce();
});

test("stopped sessions show zero signal while preserving source errors", () => {
  renderLive({
    snapshot: snapshot({
      phase: "completed",
      sources: [
        { ...snapshot().sources[0], signal_level: 0.9, connected: false, error: null },
        { ...snapshot().sources[1], signal_level: 0.8, connected: false, error: "System audio failed" },
      ],
    }),
  });
  expect(screen.getByLabelText("Live meeting state: Completed")).toBeInTheDocument();
  expect(screen.getByLabelText("Channel health")).toHaveTextContent("Stopped");
  expect(screen.getByLabelText("Channel health")).toHaveTextContent("System audio failed");
  expect(screen.getByLabelText("Jag · mikrofon signal level")).toHaveValue(0);
  expect(screen.getByLabelText("Mötet · systemljud signal level")).toHaveValue(0);
});

test("shows measured text delay and queue seconds", () => {
  renderLive({ snapshot: snapshot({ sources: [{ ...snapshot().sources[0], text_delay_ms: 800, queued_ms: 0 }, snapshot().sources[1]] }) });
  expect(screen.getByLabelText("Channel health")).toHaveTextContent("Text delay: 0.8 s");
  expect(screen.getByLabelText("Channel health")).toHaveTextContent("Queue: 0.0 s");
  expect(screen.getByLabelText("Channel health")).not.toHaveTextContent("Nyss");
});

test("shows an incomplete audio banner with at most the latest twenty gaps", async () => {
  const gaps = Array.from({ length: 21 }, (_, index) => ({
    source: index % 2 === 0 ? ("microphone" as const) : ("system" as const),
    start_ms: index * 1_000,
    end_ms: index * 1_000 + 400,
    reason: `Gap reason ${index}`,
  }));
  renderLive({ snapshot: snapshot({ gaps }) });
  const banner = screen.getByRole("status", { name: "Incomplete audio" });
  expect(banner).toHaveTextContent("Incomplete audio · 21 gaps");
  const user = userEvent.setup();
  await user.click(screen.getByText("Show 20 latest gaps", { exact: true }));
  expect(banner.getElementsByTagName("li")).toHaveLength(20);
  expect(banner).toHaveTextContent("1 earlier gaps are omitted here; all gaps remain in exports.");
  expect(banner).toHaveTextContent("Gap reason 20");
  expect(banner).not.toHaveTextContent("Gap reason 0");
});

test("a failed text export leaves audio export available for retry", async () => {
  const onSaveText = vi.fn(async (): Promise<void> => {
    throw new Error("text export failed");
  });
  const callbacks = renderLive({ snapshot: snapshot({ phase: "completed" }), onSaveText });
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Save text" }));
  await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("text export failed"));
  expect(callbacks.onSaveAudio).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Save audio" })).toBeEnabled();
});

test("shows the waiting state when all transcript rows are blank", () => {
  renderLive({
    snapshot: snapshot({
      segments: [
        { id: "empty-final", source: "microphone", start_ms: 0, end_ms: 500, text: " \t ", status: "final", revision: 2 },
        { id: "empty-preview", source: "system", start_ms: 500, end_ms: 900, text: "\n", status: "preview", revision: 1 },
      ],
    }),
  });

  const transcript = screen.getByRole("log");
  expect(transcript).toHaveTextContent("Waiting for speech…");
  expect(within(transcript).queryAllByRole("article")).toHaveLength(0);
  expect(screen.queryByRole("button", { name: "Previous" })).not.toBeInTheDocument();
});

test("counts only nonblank rows for transcript pagination", async () => {
  const segments = [
    ...Array.from({ length: 201 }, (_, index) => ({
      id: `segment-${index}`,
      source: "microphone" as const,
      start_ms: index * 1_000,
      end_ms: index * 1_000 + 900,
      text: `line ${index}`,
      status: "final" as const,
      revision: index + 1,
    })),
    ...Array.from({ length: 4 }, (_, index) => ({
      id: `blank-${index}`,
      source: "system" as const,
      start_ms: index * 1_000,
      end_ms: index * 1_000 + 100,
      text: "  ",
      status: "final" as const,
      revision: index + 1,
    })),
  ];
  const user = userEvent.setup();
  renderLive({ snapshot: snapshot({ segments }) });

  expect(screen.getByText("Page 2 of 2 · 201 segments")).toBeInTheDocument();
  expect(screen.getByText("line 200")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Previous" }));
  const transcript = screen.getByRole("log");
  expect(within(transcript).queryAllByRole("article")).toHaveLength(200);
  expect(transcript).not.toHaveTextContent("blank-");
});

test("removes stale preview text when its final row is empty", async () => {
  const view = renderLive({
    snapshot: snapshot({
      segments: [
        { id: "segment-1", source: "microphone", start_ms: 0, end_ms: 900, text: "stale preview", status: "preview", revision: 1 },
      ],
    }),
  });

  const transcript = screen.getByRole("log");
  expect(transcript).toHaveTextContent("stale preview");

  await view.rerender({
    snapshot: snapshot({
      revision: 2,
      segments: [{ id: "segment-1", source: "microphone", start_ms: 0, end_ms: 900, text: "   ", status: "final", revision: 2 }],
    }),
    busy: false,
    supported: true,
    error: null,
    onStart: view.onStart,
    onStop: view.onStop,
    onDiscard: view.onDiscard,
    onSaveText: view.onSaveText,
    onSaveAudio: view.onSaveAudio,
    onRefresh: view.onRefresh,
  });

  expect(transcript).not.toHaveTextContent("stale preview");
  expect(transcript).toHaveTextContent("Waiting for speech…");
});

test("discard uses an in-UI confirmation step", async () => {
  const callbacks = renderLive({ snapshot: snapshot({ phase: "failed" }) });
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Discard" }));
  expect(callbacks.onDiscard).not.toHaveBeenCalled();
  expect(screen.getByRole("group", { name: "Confirm discard" })).toHaveTextContent("Discard temporary audio and text?");
  await user.click(screen.getByRole("button", { name: "Yes, discard" }));
  expect(callbacks.onDiscard).toHaveBeenCalledOnce();
});

test("bounds the visible transcript and pages to earlier lines", async () => {
  const segments = Array.from({ length: 205 }, (_, index) => ({
    id: `segment-${index}`,
    source: "microphone" as const,
    start_ms: index * 1000,
    end_ms: index * 1000 + 900,
    text: `line ${index}`,
    status: "final" as const,
    revision: index,
  }));
  const user = userEvent.setup();
  renderLive({ snapshot: snapshot({ segments }) });
  expect(screen.getByRole("button", { name: "Previous" })).toBeInTheDocument();
  expect(screen.queryByText("line 0")).not.toBeInTheDocument();
  expect(screen.getByText("line 204")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Previous" }));
  expect(screen.getByText("line 0")).toBeInTheDocument();
  expect(within(screen.getByRole("log")).getAllByRole("article")).toHaveLength(200);
});

test("never renders more than 200 rows while navigating a long transcript", async () => {
  const segments = Array.from({ length: 451 }, (_, index) => ({
    id: `segment-${index}`,
    source: "microphone" as const,
    start_ms: index * 1000,
    end_ms: index * 1000 + 900,
    text: `long line ${index}`,
    status: "final" as const,
    revision: index + 1,
  }));
  const user = userEvent.setup();
  const view = renderLive({ snapshot: snapshot({ segments }) });
  const transcript = () => within(screen.getByRole("log")).getAllByRole("article");

  expect(transcript()).toHaveLength(51);
  expect(screen.getByText("long line 450")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Previous" }));
  expect(transcript()).toHaveLength(200);
  await user.click(screen.getByRole("button", { name: "Previous" }));
  expect(transcript()).toHaveLength(200);
  expect(screen.getByText("long line 0")).toBeInTheDocument();
  await view.rerender({
    snapshot: snapshot({
      revision: 2,
      segments: [
        ...segments,
        {
          id: "segment-451",
          source: "microphone" as const,
          start_ms: 451_000,
          end_ms: 451_900,
          text: "appended line",
          status: "final" as const,
          revision: 452,
        },
      ],
    }),
    busy: false,
    supported: true,
    error: null,
    onStart: view.onStart,
    onStop: view.onStop,
    onDiscard: view.onDiscard,
    onSaveText: view.onSaveText,
    onSaveAudio: view.onSaveAudio,
    onRefresh: view.onRefresh,
  });
  expect(transcript()).toHaveLength(200);
  expect(screen.getByText("long line 0")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Next" }));
  expect(transcript()).toHaveLength(200);
  expect(transcript().length).toBeLessThanOrEqual(200);
  await user.click(screen.getByRole("button", { name: "Return to latest" }));
  expect(transcript()).toHaveLength(52);
  expect(screen.getByText("appended line")).toBeInTheDocument();
});

test("follows new transcript content until the user pauses, then resumes on request", async () => {
  const view = renderLive({
    snapshot: snapshot({
      segments: [{ id: "segment-1", source: "microphone", start_ms: 0, end_ms: 900, text: "first", status: "final", revision: 1 }],
    }),
  });
  const transcript = screen.getByRole("log");
  Object.defineProperty(transcript, "scrollHeight", { configurable: true, value: 420 });
  transcript.scrollTop = 7;

  await view.rerender({
    snapshot: snapshot({
      revision: 2,
      segments: [
        { id: "segment-1", source: "microphone", start_ms: 0, end_ms: 900, text: "updated preview", status: "preview", revision: 2 },
        { id: "segment-2", source: "microphone", start_ms: 900, end_ms: 1800, text: "second", status: "final", revision: 2 },
      ],
    }),
    busy: false,
    supported: true,
    error: null,
    onStart: view.onStart,
    onStop: view.onStop,
    onDiscard: view.onDiscard,
    onSaveText: view.onSaveText,
    onSaveAudio: view.onSaveAudio,
    onRefresh: view.onRefresh,
  });
  await waitFor(() => expect(transcript.scrollTop).toBe(420));

  const user = userEvent.setup();
  await user.click(screen.getByRole("checkbox", { name: "Follow latest" }));
  transcript.scrollTop = 33;
  await view.rerender({
    snapshot: snapshot({
      revision: 3,
      segments: [
        { id: "segment-1", source: "microphone", start_ms: 0, end_ms: 900, text: "updated preview", status: "preview", revision: 2 },
        { id: "segment-2", source: "microphone", start_ms: 900, end_ms: 1800, text: "third", status: "final", revision: 3 },
      ],
    }),
    busy: false,
    supported: true,
    error: null,
    onStart: view.onStart,
    onStop: view.onStop,
    onDiscard: view.onDiscard,
    onSaveText: view.onSaveText,
    onSaveAudio: view.onSaveAudio,
    onRefresh: view.onRefresh,
  });
  expect(transcript.scrollTop).toBe(33);

  await user.click(screen.getByRole("button", { name: "Return to latest" }));
  expect(transcript.scrollTop).toBe(420);
});

test("refining sessions show determinate progress, preserve live text, and stop meters", () => {
  renderLive({
    snapshot: snapshot({
      phase: "refining",
      refinement: { status: "running", progress_percent: 42, error: null },
      segments: [{ id: "live-1", source: "microphone", start_ms: 0, end_ms: 900, text: "live text", status: "final", revision: 1 }],
    }),
  });

  expect(screen.getByLabelText("Live meeting state: Refining transcript")).toBeInTheDocument();
  expect(screen.getByRole("progressbar", { name: "Transcript refinement progress" })).toHaveAttribute("aria-valuenow", "42");
  expect(screen.getByText("42%"), "refinement progress should be visible").toBeInTheDocument();
  expect(screen.getByText(/recording stopped/i)).toBeInTheDocument();
  expect(screen.getByText(/refining locally from both tracks/i)).toBeInTheDocument();
  expect(screen.getByRole("log")).toHaveTextContent("live text");
  expect(screen.getByLabelText("Jag · mikrofon signal level")).toHaveValue(0);
  expect(screen.getByLabelText("Mötet · systemljud signal level")).toHaveValue(0);
  expect(screen.getByRole("button", { name: "Cancel refinement" })).toBeEnabled();
  expect(screen.queryByRole("button", { name: "Save text" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Discard" })).not.toBeInTheDocument();
});

test("refining uses the stop callback to cancel and keep the live transcript", async () => {
  const callbacks = renderLive({
    snapshot: snapshot({
      phase: "refining",
      refinement: { status: "running", progress_percent: 10, error: null },
    }),
  });

  await userEvent.setup().click(screen.getByRole("button", { name: "Cancel refinement" }));
  expect(callbacks.onStop).toHaveBeenCalledOnce();
});

test("completed refinement presents the final transcript after the snapshot replacement", async () => {
  const view = renderLive({
    snapshot: snapshot({
      phase: "refining",
      refinement: { status: "running", progress_percent: 99, error: null },
      segments: [{ id: "live-1", source: "microphone", start_ms: 0, end_ms: 900, text: "live text", status: "final", revision: 1 }],
    }),
  });
  expect(screen.getByRole("log")).toHaveTextContent("live text");

  await view.rerender({
    snapshot: snapshot({
      phase: "completed",
      refinement: { status: "completed", progress_percent: 100, error: null },
      segments: [{ id: "final-1", source: "microphone", start_ms: 0, end_ms: 900, text: "final text", status: "final", revision: 2 }],
    }),
    busy: false,
    supported: true,
    error: null,
    onStart: view.onStart,
    onStop: view.onStop,
    onDiscard: view.onDiscard,
    onSaveText: view.onSaveText,
    onSaveAudio: view.onSaveAudio,
    onRefresh: view.onRefresh,
  });

  expect(screen.getByLabelText("Live meeting state: Final transcript")).toBeInTheDocument();
  expect(screen.getByRole("log")).toHaveTextContent("final text");
  expect(screen.getByRole("log")).not.toHaveTextContent("live text");
  expect(screen.getByRole("button", { name: "Save text" })).toBeInTheDocument();
});

test("cancelled and failed refinement keep the live transcript label and surface safe errors", async () => {
  const view = renderLive({
    snapshot: snapshot({
      phase: "completed",
      refinement: { status: "cancelled", progress_percent: 58, error: null },
      error: "Refinement cancelled; live transcript kept",
      segments: [{ id: "live-1", source: "microphone", start_ms: 0, end_ms: 900, text: "live text", status: "final", revision: 1 }],
    }),
  });
  expect(screen.getByLabelText("Live meeting state: Live transcript kept")).toBeInTheDocument();
  expect(screen.getByRole("alert")).toHaveTextContent("Refinement cancelled; live transcript kept");

  await view.rerender({
    snapshot: snapshot({
      phase: "completed",
      refinement: { status: "failed", progress_percent: 58, error: "Refinement unavailable" },
      error: null,
      segments: [{ id: "live-1", source: "microphone", start_ms: 0, end_ms: 900, text: "live text", status: "final", revision: 1 }],
    }),
    busy: false,
    supported: true,
    error: null,
    onStart: view.onStart,
    onStop: view.onStop,
    onDiscard: view.onDiscard,
    onSaveText: view.onSaveText,
    onSaveAudio: view.onSaveAudio,
    onRefresh: view.onRefresh,
  });
  expect(screen.getByLabelText("Live meeting state: Live transcript kept")).toBeInTheDocument();
  expect(screen.getByRole("alert")).toHaveTextContent("Refinement unavailable");
});

test("legacy terminal snapshots keep their lifecycle labels without refinement metadata", async () => {
  const view = renderLive({ snapshot: snapshot({ phase: "completed", refinement: null }) });
  expect(screen.getByLabelText("Live meeting state: Completed")).toBeInTheDocument();

  await view.rerender({
    snapshot: snapshot({ phase: "cancelled" }),
    busy: false,
    supported: true,
    error: null,
    onStart: view.onStart,
    onStop: view.onStop,
    onDiscard: view.onDiscard,
    onSaveText: view.onSaveText,
    onSaveAudio: view.onSaveAudio,
    onRefresh: view.onRefresh,
  });
  expect(screen.getByLabelText("Live meeting state: Cancelled")).toBeInTheDocument();

  await view.rerender({
    snapshot: snapshot({ phase: "failed" }),
    busy: false,
    supported: true,
    error: null,
    onStart: view.onStart,
    onStop: view.onStop,
    onDiscard: view.onDiscard,
    onSaveText: view.onSaveText,
    onSaveAudio: view.onSaveAudio,
    onRefresh: view.onRefresh,
  });
  expect(screen.getByLabelText("Live meeting state: Failed")).toBeInTheDocument();
});

test("explains automatic local refinement in the live meeting copy", () => {
  renderLive();
  expect(screen.getByText(/stopping starts local refinement from both tracks/i)).toBeInTheDocument();
});
