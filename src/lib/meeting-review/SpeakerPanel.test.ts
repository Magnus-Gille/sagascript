import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import type { ComponentProps } from "svelte";
import SpeakerPanel from "./SpeakerPanel.svelte";
import type { MeetingTranscript } from "../meeting-types";

const transcript: MeetingTranscript = {
  schema_version: 1,
  source_sha256: "84e029fa9fd68ed5510cb4e8f1a0b811abfc4b60c4a9c1e7a80552296107c83a",
  language: "en",
  model: "test-model",
  duration_seconds: 8,
  segments: [],
  speakers: [
    { id: "speaker-1", label: "Alice" },
    { id: "speaker-2", label: "Bo" },
  ],
};

function renderPanel(overrides: Partial<ComponentProps<typeof SpeakerPanel>> = {}) {
  const props: ComponentProps<typeof SpeakerPanel> = {
    idPrefix: "",
    transcript,
    labelDrafts: {},
    mergeTargets: {},
    disabled: () => false,
    renameSpeaker: vi.fn(async () => undefined),
    mergeSpeaker: vi.fn(async () => undefined),
    discardMerge: vi.fn(),
    setLabelDraft: vi.fn(),
    setMergeTarget: vi.fn(),
    ...overrides,
  };
  render(SpeakerPanel, { props });
  return props;
}

test("a chosen merge target is reported, and Merge sends the speaker to merge", async () => {
  const user = userEvent.setup();
  const props = renderPanel({ mergeTargets: { "speaker-2": "speaker-1" } });

  await user.selectOptions(screen.getByRole("combobox", { name: "Merge Alice into" }), "speaker-2");
  expect(props.setMergeTarget).toHaveBeenCalledWith("speaker-1", "speaker-2");

  await user.click(screen.getAllByRole("button", { name: "Merge" })[1]);
  expect(props.mergeSpeaker).toHaveBeenCalledWith("speaker-2");
});

test("typing a name reports the draft, and Enter renames that speaker", async () => {
  const user = userEvent.setup();
  const props = renderPanel();

  await user.type(screen.getByRole("textbox", { name: "Rename Bo" }), "x{Enter}");
  expect(props.setLabelDraft).toHaveBeenCalledWith("speaker-2", "Box");
  expect(props.renameSpeaker).toHaveBeenCalledWith(transcript.speakers[1]);
});

test("a single speaker has no merge controls, and no speakers shows the empty note", () => {
  const view = render(SpeakerPanel, {
    props: {
      idPrefix: "",
      transcript: { ...transcript, speakers: [transcript.speakers[0]] },
      labelDrafts: {},
      mergeTargets: {},
      disabled: () => true,
      renameSpeaker: vi.fn(async () => undefined),
      mergeSpeaker: vi.fn(async () => undefined),
      discardMerge: vi.fn(),
      setLabelDraft: vi.fn(),
      setMergeTarget: vi.fn(),
    },
  });
  expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Rename Alice" })).toBeDisabled();
  view.unmount();

  renderPanel({ transcript: { ...transcript, speakers: [] } });
  expect(screen.getByText("No speaker labels are available for this transcript.")).toBeInTheDocument();
});
