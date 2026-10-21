import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import type { ComponentProps } from "svelte";
import MeetingReview from "./MeetingReview.svelte";
import type { MeetingReview as Review, MeetingTranscript } from "./meeting-types";
import { deferred } from "../test/deferred";

const transcript: MeetingTranscript = {
  schema_version: 1,
  source_sha256: "meeting-source",
  language: "en",
  model: "test-model",
  duration_seconds: 8,
  segments: [{ id: "segment-1", start: 0, end: 4, text: "Original words", speaker: "speaker-1" }],
  speakers: [
    { id: "speaker-1", label: "Alice" },
    { id: "speaker-2", label: "Bo" },
  ],
};

const review: Review = {
  schema_version: 1,
  original: transcript,
  original_revision: "original-revision",
  generation: 1,
  batches: [],
  revision: "review-revision",
};

function renderReview(overrides: Partial<ComponentProps<typeof MeetingReview>> = {}) {
  const props: ComponentProps<typeof MeetingReview> = {
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
  };
  const view = render(MeetingReview, { props });
  return view;
}

test("Rename applies the speaker label through the public correction callback", async () => {
  const onApply = vi.fn(async () => undefined);
  renderReview({ onApply });
  const user = userEvent.setup();

  await user.clear(screen.getByRole("textbox", { name: "Rename Alice" }));
  await user.type(screen.getByRole("textbox", { name: "Rename Alice" }), "Alicia");
  await user.click(screen.getAllByRole("button", { name: "Rename" })[0]);

  await waitFor(() => expect(onApply).toHaveBeenCalledWith([{ kind: "rename_speaker", speaker_id: "speaker-1", label: "Alicia" }]));
});

test("Discard restores committed segment values, then text and speaker corrections apply", async () => {
  const onApply = vi.fn(async () => undefined);
  renderReview({ onApply });
  const user = userEvent.setup();
  const text = screen.getByRole("textbox", { name: "Edit transcript segment at 0:00" });
  const speaker = screen.getByRole("combobox", { name: "Edit speaker for 0:00" });

  await user.clear(text);
  await user.type(text, "Unapplied text");
  await user.selectOptions(speaker, "speaker-2");
  await user.click(screen.getByRole("button", { name: "Discard" }));

  expect(text).toHaveValue("Original words");
  expect(speaker).toHaveValue("speaker-1");
  expect(onApply).not.toHaveBeenCalled();

  await user.clear(text);
  await user.type(text, "Corrected words");
  await user.selectOptions(speaker, "speaker-2");
  await user.click(screen.getByRole("button", { name: "Apply" }));

  await waitFor(() =>
    expect(onApply).toHaveBeenCalledWith([
      { kind: "edit_segment", segment_id: "segment-1", text: "Corrected words", speaker_id: "speaker-2" },
    ]),
  );

  expect(onApply).toHaveBeenCalledTimes(1);
});

test("clearing a merge selection leaves it unapplied", async () => {
  const onApply = vi.fn(async () => undefined);
  renderReview({ onApply });
  const user = userEvent.setup();
  const target = screen.getByRole("combobox", { name: "Merge Alice into" });

  await user.selectOptions(target, "speaker-2");
  expect(screen.getAllByRole("button", { name: "Merge" })[0]).toBeEnabled();
  await user.click(screen.getByRole("button", { name: "Clear merge selection for Alice" }));

  expect(target).toHaveValue("");
  expect(screen.getAllByRole("button", { name: "Merge" })[0]).toBeDisabled();
  expect(onApply).not.toHaveBeenCalled();
});

test("Save and each export button call the matching callback, and false results show no success notice", async () => {
  const onSave = vi.fn(async () => false);
  const onExport = vi.fn(async () => false);
  renderReview({ onSave, onExport });
  const user = userEvent.setup();

  await user.click(screen.getByRole("button", { name: "Save review" }));
  await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(screen.getByRole("button", { name: "Save review" })).toBeEnabled());
  expect(screen.queryByText("Review saved.", { exact: true })).not.toBeInTheDocument();

  const formats = [
    ["Plain text", "plain"],
    ["Markdown", "markdown"],
    ["Review JSON", "json"],
    ["SRT subtitles", "srt"],
    ["WebVTT subtitles", "vtt"],
  ] as const;
  for (const [label, format] of formats) {
    await user.click(screen.getByRole("button", { name: label }));
    await waitFor(() => expect(onExport).toHaveBeenLastCalledWith(format));
    await waitFor(() => expect(screen.getByRole("button", { name: label })).toBeEnabled());
    expect(screen.queryByText("Export saved.", { exact: true })).not.toBeInTheDocument();
  }

  expect(onExport).toHaveBeenCalledTimes(formats.length);
});

test("a rejected save shows its diagnostic and a later successful save recovers", async () => {
  const onSave = vi.fn().mockRejectedValueOnce(new Error("Disk is full.")).mockResolvedValue(true);
  renderReview({ onSave });
  const user = userEvent.setup();

  await user.click(screen.getByRole("button", { name: "Save review" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Disk is full.");
  await waitFor(() => expect(screen.getByRole("button", { name: "Save review" })).toBeEnabled());
  expect(screen.queryByText("Review saved.", { exact: true })).not.toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Save review" }));
  expect(await screen.findByRole("status")).toHaveTextContent("Review saved.");
  expect(onSave).toHaveBeenCalledTimes(2);
});

test("dirty drafts and parent busy state block persistence, with an idle save still available", async () => {
  const onSave = vi.fn(async () => true);
  const onExport = vi.fn(async () => true);
  const view = renderReview({ onSave, onExport });
  const user = userEvent.setup();
  const text = screen.getByRole("textbox", { name: "Edit transcript segment at 0:00" });

  await user.clear(text);
  await user.type(text, "Unapplied words");
  await waitFor(() => expect(screen.getByRole("button", { name: "Save review" })).toBeDisabled());
  expect(
    screen.getByText("Unapplied edits are not included in saves/exports. Apply or discard them before saving or exporting."),
  ).toBeInTheDocument();
  const exportLabels = ["Plain text", "Markdown", "Review JSON", "SRT subtitles", "WebVTT subtitles"];
  for (const label of exportLabels) expect(screen.getByRole("button", { name: label })).toBeDisabled();
  expect(onSave).not.toHaveBeenCalled();
  expect(onExport).not.toHaveBeenCalled();

  await user.click(screen.getByRole("button", { name: "Discard" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Save review" })).toBeEnabled());
  for (const label of exportLabels) expect(screen.getByRole("button", { name: label })).toBeEnabled();
  await user.click(screen.getByRole("button", { name: "Save review" }));
  expect(await screen.findByRole("status")).toHaveTextContent("Review saved.");
  expect(onSave).toHaveBeenCalledTimes(1);

  view.unmount();
  const onDraftDirtyChange = vi.fn();
  const onBusySave = vi.fn(async () => true);
  const onBusyExport = vi.fn(async () => true);
  renderReview({ busy: true, onDraftDirtyChange, onSave: onBusySave, onExport: onBusyExport });
  await waitFor(() => expect(onDraftDirtyChange).toHaveBeenCalledWith(false));
  expect(screen.getByRole("button", { name: "Save review" })).toBeDisabled();
  for (const label of exportLabels) expect(screen.getByRole("button", { name: label })).toBeDisabled();
  expect(onBusySave).not.toHaveBeenCalled();
  expect(onBusyExport).not.toHaveBeenCalled();
});

test("a pending persistence action disables Save and every export until it settles", async () => {
  const save = deferred<boolean>();
  const onSave = vi.fn(() => save.promise);
  renderReview({ onSave });
  const user = userEvent.setup();

  await user.click(screen.getByRole("button", { name: "Save review" }));
  const saving = await screen.findByRole("button", { name: "Saving…" });
  expect(saving).toBeDisabled();
  for (const label of ["Plain text", "Markdown", "Review JSON", "SRT subtitles", "WebVTT subtitles"]) {
    expect(screen.getByRole("button", { name: label })).toBeDisabled();
  }

  save.resolve(true);
  expect(await screen.findByRole("status")).toHaveTextContent("Review saved.");
  await waitFor(() => expect(screen.getByRole("button", { name: "Save review" })).toBeEnabled());
});
