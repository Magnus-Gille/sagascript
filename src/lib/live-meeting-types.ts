/** The two input channels exposed by the live meeting transport. */
export type LiveSource = "microphone" | "system";

/** A live meeting's lifecycle is monotonic apart from terminal snapshots. */
export type LivePhase = "starting" | "running" | "draining" | "completed" | "cancelled" | "failed";

export type LiveSegmentStatus = "preview" | "final";

/** Exact wire shape for a transcript segment. */
export interface LiveSegment {
  id: string;
  source: LiveSource;
  start_ms: number;
  end_ms: number;
  text: string;
  status: LiveSegmentStatus;
  revision: number;
}

/** Exact wire shape for one input channel's health counters. */
export interface SourceStatus {
  source: LiveSource;
  connected: boolean;
  signal_level: number;
  last_audio_ms: number | null;
  last_text_ms: number | null;
  text_delay_ms: number | null;
  queued_ms: number;
  dropped_frames: number;
  error: string | null;
}

/** A period where no audio was available for a channel. */
export interface Gap {
  source: LiveSource;
  start_ms: number;
  end_ms: number;
  reason: string;
}

/** Exact wire shape published by the live meeting backend. */
export interface LiveSnapshot {
  id: string;
  revision: number;
  phase: LivePhase;
  elapsed_ms: number;
  sources: SourceStatus[];
  segments: LiveSegment[];
  gaps: Gap[];
  error: string | null;
  audio_available: boolean;
}
