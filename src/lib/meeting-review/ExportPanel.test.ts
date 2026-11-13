import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import ExportPanel from "./ExportPanel.svelte";
import type { DiarizationReport } from "../meeting-types";

const report: DiarizationReport = {
  schema_version: 1,
  source_sha256: "84e029fa9fd68ed5510cb4e8f1a0b811abfc4b60c4a9c1e7a80552296107c83a",
  duration_seconds: 10,
  build_revision: "test-revision",
  build_version: "test-version",
  asr_segments: [],
  transcript_modified: false,
  parameters: {},
  activity: [],
  regions: [],
  attributions: [],
};

const limitation = /cannot represent simultaneous acoustic activity/;

test("an acoustic report adds the export limitation, and a pending export relabels its own button", () => {
  render(ExportPanel, {
    props: {
      idPrefix: "",
      acousticReport: report,
      pendingAction: "export-srt",
      persistenceDisabled: () => true,
      exportReview: vi.fn(async () => undefined),
    },
  });

  expect(screen.getByText(limitation)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Exporting…" })).toBeDisabled();
  expect(screen.queryByRole("button", { name: "SRT subtitles" })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Plain text" })).toBeDisabled();
});

test("without an acoustic report there is no limitation note, and a click exports that format", async () => {
  const exportReview = vi.fn(async () => undefined);
  render(ExportPanel, {
    props: { idPrefix: "", acousticReport: null, pendingAction: null, persistenceDisabled: () => false, exportReview },
  });

  expect(screen.queryByText(limitation)).not.toBeInTheDocument();
  await userEvent.setup().click(screen.getByRole("button", { name: "WebVTT subtitles" }));
  expect(exportReview).toHaveBeenCalledWith("vtt");
});
