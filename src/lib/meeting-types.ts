export interface MeetingSpeaker {
  id: string;
  label: string;
}

export interface MeetingSegment {
  id: string;
  start: number;
  end: number;
  text: string;
  speaker: string;
}

export interface MeetingTranscript {
  schema_version: number;
  source_sha256: string;
  language: string;
  model: string;
  duration_seconds: number;
  segments: MeetingSegment[];
  speakers: MeetingSpeaker[];
}

export type MeetingExportFormat = "plain" | "markdown" | "json" | "srt" | "vtt";
