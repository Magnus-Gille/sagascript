import { fireEvent, render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import type { ComponentProps } from "svelte";
import MeetingReview from "./MeetingReview.svelte";
import type { DiarizationReport, MeetingReview as Review, MeetingTranscript } from "./meeting-types";

const convertFileSrc = vi.hoisted(() => vi.fn((token: string, protocol?: string) => `asset://${protocol}/${token}`));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc }));

const transcript: MeetingTranscript = {
  schema_version: 1,
  source_sha256: "84e029fa9fd68ed5510cb4e8f1a0b811abfc4b60c4a9c1e7a80552296107c83a",
  language: "en",
  model: "test-model",
  duration_seconds: 200,
  segments: [{ id: "segment-1", start: 0, end: 2, text: "Original words", speaker: "speaker-1" }],
  speakers: [
    { id: "speaker-1", label: "Alice" },
    { id: "speaker-2", label: "Bo" },
  ],
};

const review: Review = {
  schema_version: 1,
  original: transcript,
  original_revision: "ff19c40d1243fa53db0fd782e432d3dcb65a1380cefa4bd3320b30f8ae4b459a",
  generation: 0,
  batches: [],
  revision: "f3bec86d9f1132df14fa67413303779e30d9277868d31015ed95d7fd9a167e03",
};

function acousticReport(overrides: Partial<DiarizationReport> = {}): DiarizationReport {
  return {
    schema_version: 1,
    source_sha256: transcript.source_sha256,
    duration_seconds: transcript.duration_seconds,
    build_revision: "test-revision",
    build_version: "test-version",
    decoder: null,
    asr_segments: [],
    transcript_modified: true,
    parameters: {},
    activity: Array.from({ length: 102 }, (_, index) => ({
      start: index,
      end: index + 0.5,
      speakers: ["speaker-1", "speaker-2"],
    })),
    regions: [],
    attributions: [],
    ...overrides,
  };
}

function renderReview(overrides: Partial<ComponentProps<typeof MeetingReview>> = {}) {
  return render(MeetingReview, {
    props: {
      review,
      transcript,
      initialDraftSnapshot: null,
      onApply: vi.fn(async () => undefined),
      onUndo: vi.fn(async () => undefined),
      onReset: vi.fn(async () => undefined),
      onSave: vi.fn(async () => true),
      onExport: vi.fn(async () => true),
      onAttachAudio: vi.fn(async () => null),
      onDetachAudio: vi.fn(async () => undefined),
      ...overrides,
    },
  });
}

let playSpy: ReturnType<typeof vi.spyOn>;

afterEach(() => {
  playSpy?.mockRestore();
  vi.unstubAllGlobals();
});

test("renders segment edits against the original transcript and caps visible overlap spans at 100", () => {
  const reviewedTranscript = {
    ...transcript,
    segments: [{ ...transcript.segments[0], text: "Corrected words" }],
    diarization: acousticReport(),
  };
  renderReview({ transcript: reviewedTranscript });

  expect(screen.getByText("1 segment, ordered by time.")).toBeInTheDocument();
  expect(
    screen.getByText(
      "Simultaneous speaker activity appears in 102 intervals (51.00 seconds). Transcript words remain shown once; this panel does not duplicate words for overlapping speakers.",
    ),
  ).toBeInTheDocument();
  expect(screen.getByRole("list", { name: "Simultaneous speaker activity" }).querySelectorAll('[role="listitem"]')).toHaveLength(100);
  expect(screen.getByText("2 additional overlap intervals were omitted from this view.")).toBeInTheDocument();
  expect(screen.getByText("Transcript edited")).toBeInTheDocument();
  expect(screen.getByText("Edited")).toBeInTheDocument();
});

test("explains absent diagnostic evidence and describes activity without claiming calibrated confidence", () => {
  const report = acousticReport({ diagnostics_included: false, activity: [{ start: 0, end: 1, speakers: ["speaker-1", "speaker-2"] }] });
  renderReview({ transcript: { ...transcript, diarization: report } });

  expect(screen.getByText(/Additional diagnostic evidence was not included for this run\./)).toBeInTheDocument();
  expect(screen.getByText(/activity and any included attribution evidence are not calibrated speaker confidence/i)).toBeInTheDocument();
  expect(screen.getByText(/Transcript words remain shown once/)).toBeInTheDocument();
  expect(screen.queryByText(/speaker confidence[^\n]*%/i)).not.toBeInTheDocument();
});

test("audio, persistence, undo, and reset flows avoid network, storage, recording, and autoplay APIs", async () => {
  const fetchSpy = vi.fn();
  const storage = {
    getItem: vi.fn(),
    setItem: vi.fn(),
    removeItem: vi.fn(),
    clear: vi.fn(),
  };
  const audioContext = vi.fn();
  const mediaRecorder = vi.fn();
  const confirmSpy = vi.fn(() => true);
  const onSave = vi.fn(async () => true);
  const onExport = vi.fn(async () => true);
  const onUndo = vi.fn(async () => undefined);
  const onReset = vi.fn(async () => undefined);
  const onDetachAudio = vi.fn(async () => undefined);
  playSpy = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
  vi.stubGlobal("fetch", fetchSpy);
  vi.stubGlobal("localStorage", storage);
  vi.stubGlobal("AudioContext", audioContext);
  vi.stubGlobal("MediaRecorder", mediaRecorder);
  vi.stubGlobal("confirm", confirmSpy);

  const reviewWithHistory = {
    ...review,
    batches: [{ operations: [{ kind: "edit_segment" as const, segment_id: "segment-1", text: "Previously edited" }] }],
  };
  renderReview({
    review: reviewWithHistory,
    transcript: { ...transcript, diarization: acousticReport({ activity: [] }) },
    onSave,
    onExport,
    onUndo,
    onReset,
    onAttachAudio: vi.fn(async () => ({ token: "opaque-meeting-token", mime: "audio/wav" })),
    onDetachAudio,
  });
  const user = userEvent.setup();

  await user.click(screen.getByRole("button", { name: "Attach audio" }));
  const audio = screen.getByLabelText("Meeting audio") as HTMLAudioElement;
  await user.click(screen.getByRole("button", { name: "Seek to 0:00" }));
  await user.clear(screen.getByRole("textbox", { name: "Edit transcript segment at 0:00" }));
  await user.type(screen.getByRole("textbox", { name: "Edit transcript segment at 0:00" }), "Reviewed words");
  await user.click(screen.getByRole("button", { name: "Discard" }));
  await user.click(screen.getByRole("button", { name: "Save review" }));
  await user.click(screen.getByRole("button", { name: "Plain text" }));
  await user.click(screen.getByRole("button", { name: "Undo" }));
  await user.click(screen.getByRole("button", { name: "Reset" }));
  await user.click(screen.getByRole("button", { name: "Detach" }));
  await fireEvent.keyDown(window, { key: "ArrowDown" });

  expect(audio).not.toBeInTheDocument();
  expect(onSave).toHaveBeenCalledTimes(1);
  expect(onExport).toHaveBeenCalledWith("plain");
  expect(onUndo).toHaveBeenCalledTimes(1);
  expect(onReset).toHaveBeenCalledTimes(1);
  expect(onDetachAudio).toHaveBeenCalledWith("opaque-meeting-token");
  expect(confirmSpy).toHaveBeenCalledTimes(1);
  expect(playSpy).not.toHaveBeenCalled();
  expect(fetchSpy).not.toHaveBeenCalled();
  expect(storage.getItem).not.toHaveBeenCalled();
  expect(storage.setItem).not.toHaveBeenCalled();
  expect(storage.removeItem).not.toHaveBeenCalled();
  expect(storage.clear).not.toHaveBeenCalled();
  expect(audioContext).not.toHaveBeenCalled();
  expect(mediaRecorder).not.toHaveBeenCalled();
});
