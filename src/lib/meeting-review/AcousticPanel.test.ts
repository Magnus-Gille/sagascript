import { render, screen } from "@testing-library/svelte";
import { expect, test } from "vitest";
import AcousticPanel from "./AcousticPanel.svelte";
import type { DiarizationReport } from "../meeting-types";

function report(overrides: Partial<DiarizationReport> = {}): DiarizationReport {
  return {
    schema_version: 1,
    source_sha256: "84e029fa9fd68ed5510cb4e8f1a0b811abfc4b60c4a9c1e7a80552296107c83a",
    duration_seconds: 10,
    build_revision: "test-revision",
    build_version: "test-version",
    decoder: null,
    diagnostics_included: true,
    asr_segments: [],
    transcript_modified: false,
    parameters: {},
    activity: [],
    regions: [],
    attributions: [],
    ...overrides,
  };
}

const labels: Record<string, string> = { "speaker-1": "Alice", "speaker-2": "Bo" };

function renderPanel(acousticReport: DiarizationReport) {
  return render(AcousticPanel, {
    props: {
      idPrefix: "",
      acousticReport,
      displayLabel: (id: string) => labels[id] ?? id,
      formatTimestamp: (seconds: number) => `t${seconds}`,
    },
  });
}

test("included diagnostics show the decoder, track fallback regions and sorted attribution reasons", () => {
  renderPanel(
    report({
      decoder: {
        strategy: "beam_search",
        beam_size: 5,
        temperature_fallback: false,
        vad_enabled: false,
        vad_threshold: 0.5,
        vad_min_silence_duration_ms: 100,
        vad_speech_pad_ms: 30,
        vad_samples_overlap: 0.1,
        timestamp_method: "token",
      },
      regions: [
        {
          index: 0,
          start: 0,
          end: 1,
          track: 0,
          speaker: "speaker-1",
          embedding_status: "usable",
          assigned_centroid_distance: null,
          nearest_other_centroid_distance: null,
          used_track_fallback: true,
        },
      ],
      attributions: [
        { index: 0, start: 0, end: 1, speaker: "speaker-1", reason: "temporal_overlap", support: [] },
        { index: 1, start: 1, end: 2, speaker: "speaker-2", reason: "nearest_gap", support: [] },
        { index: 2, start: 2, end: 3, speaker: "speaker-2", reason: "nearest_gap", support: [] },
      ],
    }),
  );

  expect(screen.getByText(/Decoder beam_search, beam 5, VAD off\./)).toBeInTheDocument();
  expect(screen.getByText("1 region used track fallback.")).toBeInTheDocument();
  expect(screen.getByText("Attribution reasons: nearest_gap (2), temporal_overlap (1).")).toBeInTheDocument();
  expect(screen.getByText("No simultaneous speaker activity was recorded in the acoustic timeline.")).toBeInTheDocument();
  expect(screen.queryByText("Transcript edited")).not.toBeInTheDocument();
});

test("a single overlap interval is labelled with speaker names and singular wording", () => {
  renderPanel(report({ activity: [{ start: 1, end: 1.25, speakers: ["speaker-1", "speaker-2"] }] }));

  expect(screen.getByRole("status")).toHaveTextContent("Simultaneous speaker activity appears in 1 interval (0.25 seconds).");
  const rows = screen.getAllByRole("listitem");
  expect(rows).toHaveLength(1);
  expect(rows[0]).toHaveTextContent("t1–t1.25");
  expect(rows[0]).toHaveTextContent("Alice, Bo");
  expect(screen.queryByLabelText("Diarization diagnostic evidence")).not.toBeInTheDocument();
});
