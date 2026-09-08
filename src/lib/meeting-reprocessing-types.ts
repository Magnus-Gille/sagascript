import type { CorrectionOperation, MeetingReview, MeetingTranscript } from "./meeting-types";

export type ReprocessingMode = "recluster" | "rediarize" | "full";
export interface RequiredWork {
  decode_audio: boolean;
  transcription: boolean;
  language_detection: boolean;
  segmentation: boolean;
  embeddings: boolean;
  clustering: boolean;
}
export interface ReprocessingPlan {
  schema_version: number;
  mode: ReprocessingMode;
  context: {
    source_sha256: string;
    previous_revision: string;
    transcription_context_sha256: string;
    analysis_context_sha256: string;
    cache_sha256: string | null;
  };
  threshold: number;
  required_work: RequiredWork;
  revision: string;
}
export interface SelectedReprocessingPlan {
  plan: ReprocessingPlan;
  file_path: string;
  cache_path: string | null;
  cache_output: string | null;
}
export interface MeetingReprocessingProposal {
  schema_version: number;
  previous: MeetingReview;
  proposed: MeetingTranscript;
  generation: number;
  resolutions: (CorrectionOperation[] | null)[];
  revision: string;
}
export interface MigrationStep {
  original: CorrectionOperation;
  mapped: CorrectionOperation[] | null;
  status: "applied" | "conflict" | "blocked";
}
export interface ProposalState {
  proposal: MeetingReprocessingProposal;
  preview: { candidate: MeetingReview; steps: MigrationStep[] };
  candidate: MeetingTranscript;
}
export interface ReprocessingResult {
  proposal: MeetingReprocessingProposal;
  required_work: RequiredWork;
  timings: {
    validation_seconds: number;
    decode_seconds: number;
    analysis_seconds: number;
    clustering_seconds: number;
    full_pipeline_seconds: number;
    proposal_seconds: number;
    total_seconds: number;
  };
}
