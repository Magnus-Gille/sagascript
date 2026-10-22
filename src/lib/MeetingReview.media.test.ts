import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { ComponentProps } from "svelte";
import MeetingReview from "./MeetingReview.svelte";
import type { MeetingReview as Review, MeetingTranscript } from "./meeting-types";

const convertFileSrc = vi.hoisted(() => vi.fn((token: string, protocol?: string) => `asset://${protocol}/${token}`));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc }));

const transcript: MeetingTranscript = {
  schema_version: 1,
  source_sha256: "84e029fa9fd68ed5510cb4e8f1a0b811abfc4b60c4a9c1e7a80552296107c83a",
  language: "en",
  model: "test-model",
  duration_seconds: 8,
  segments: [
    { id: "segment-1", start: 0, end: 2, text: "First words", speaker: "speaker-1" },
    { id: "segment-2", start: 2, end: 4, text: "Second words", speaker: "speaker-2" },
  ],
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
      onAttachAudio: vi.fn(async () => ({ token: "opaque-meeting-token", mime: "audio/wav" })),
      onDetachAudio: vi.fn(async () => undefined),
      ...overrides,
    },
  });
}

let pauseSpy: ReturnType<typeof vi.spyOn>;
let playSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  convertFileSrc.mockClear();
  pauseSpy = vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => undefined);
  playSpy = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
});

afterEach(() => {
  pauseSpy.mockRestore();
  playSpy.mockRestore();
});

async function attachAudio() {
  await userEvent.setup().click(screen.getByRole("button", { name: "Attach audio" }));
  return screen.getByLabelText("Meeting audio") as HTMLAudioElement;
}

test("pauses when inactive and detaches audio on unmount", async () => {
  const onDetachAudio = vi.fn(async () => undefined);
  const view = renderReview({ onDetachAudio });
  const user = userEvent.setup();

  expect(screen.queryByText(/Current timestamp:/)).not.toBeInTheDocument();
  expect(document.querySelector(".active-segment")).toBeNull();
  await user.click(screen.getByRole("button", { name: "Attach audio" }));
  const audio = screen.getByLabelText("Meeting audio") as HTMLAudioElement;
  await waitFor(() => expect(audio).toHaveAttribute("src", "asset://meeting-audio/opaque-meeting-token"));
  expect(convertFileSrc).toHaveBeenCalledWith("opaque-meeting-token", "meeting-audio");
  expect(audio).toHaveAttribute("preload", "metadata");

  await view.rerender({
    review,
    transcript,
    active: false,
    initialDraftSnapshot: null,
    onApply: vi.fn(async () => undefined),
    onUndo: vi.fn(async () => undefined),
    onReset: vi.fn(async () => undefined),
    onSave: vi.fn(async () => true),
    onExport: vi.fn(async () => true),
    onAttachAudio: vi.fn(async () => ({ token: "opaque-meeting-token", mime: "audio/wav" })),
    onDetachAudio,
  });
  await waitFor(() => expect(pauseSpy).toHaveBeenCalled());
  expect(pauseSpy.mock.contexts.at(-1)).toBe(audio);

  view.unmount();
  expect(onDetachAudio).toHaveBeenCalledWith("opaque-meeting-token");
});

test("time updates highlight half-open segments, and pause plus edits or timestamp seeks never autoplay", async () => {
  const view = renderReview();
  const audio = await attachAudio();
  const first = document.getElementById("meeting-segment-segment-1")!;
  const second = document.getElementById("meeting-segment-segment-2")!;

  Object.defineProperty(audio, "currentTime", { configurable: true, writable: true, value: 1.999 });
  await fireEvent.timeUpdate(audio);
  expect(first).toHaveClass("active-segment");
  expect(second).not.toHaveClass("active-segment");
  expect(screen.getByText("Current timestamp: Alice")).toBeInTheDocument();

  Object.defineProperty(audio, "paused", { configurable: true, value: true });
  Object.defineProperty(audio, "currentTime", { configurable: true, writable: true, value: 2 });
  await fireEvent.timeUpdate(audio);
  expect(first).not.toHaveClass("active-segment");
  expect(second).toHaveClass("active-segment");
  expect(screen.getByText("Current timestamp: Bo")).toBeInTheDocument();
  expect(screen.queryByText(/Playing:/)).not.toBeInTheDocument();

  const user = userEvent.setup();
  await user.clear(screen.getByRole("textbox", { name: "Edit transcript segment at 0:00" }));
  await user.type(screen.getByRole("textbox", { name: "Edit transcript segment at 0:00" }), "Changed words");
  await user.selectOptions(screen.getByRole("combobox", { name: "Edit speaker for 0:00" }), "speaker-2");
  await user.click(screen.getByRole("button", { name: "Seek to 0:00" }));
  expect(audio.currentTime).toBe(0);
  expect(playSpy).not.toHaveBeenCalled();

  await fireEvent.error(audio);
  expect(await screen.findByRole("alert")).toHaveTextContent("This audio cannot be played");
  expect(screen.getByRole("alert")).toHaveTextContent("Corrections and exports remain available.");
  view.unmount();
});

test("wheel, touch, and keyboard scrolling each turn off Follow playback", async () => {
  const user = userEvent.setup();
  const view = renderReview();
  await attachAudio();
  const checkbox = screen.getByRole("checkbox", { name: "Follow playback" });

  await user.click(checkbox);
  expect(checkbox).toBeChecked();
  await fireEvent.wheel(screen.getByRole("region", { name: "Transcript segments" }));
  expect(checkbox).not.toBeChecked();

  await user.click(checkbox);
  await fireEvent.touchMove(screen.getByRole("region", { name: "Transcript segments" }));
  expect(checkbox).not.toBeChecked();

  await user.click(checkbox);
  await fireEvent.keyDown(window, { key: "PageDown" });
  expect(checkbox).not.toBeChecked();
  expect(playSpy).not.toHaveBeenCalled();
  view.unmount();
});

test("transcript text is rendered as text rather than injected markup", async () => {
  const hostile = "<img src=x onerror=alert(1)>";
  const hostileTranscript = { ...transcript, segments: [{ ...transcript.segments[0], text: hostile }] };
  renderReview({ transcript: hostileTranscript, review: { ...review, original: hostileTranscript } });

  expect(screen.getByRole("textbox", { name: "Edit transcript segment at 0:00" })).toHaveValue(hostile);
  expect(document.querySelector("img[src='x']")).toBeNull();
});
