<script lang="ts">
  import type { MeetingJobStatus } from "./api";
  import {
    formatDuration,
    meetingPhases,
    phaseDetail,
    phaseLabel,
    statusCheckAnnouncement,
    statusCheckState,
    statusCheckMessage,
    statusTitle,
    type MeetingProgressPhase,
  } from "./meeting-progress";

  interface Props {
    status: MeetingJobStatus;
    phase: string;
    elapsedMs: number;
    phaseElapsedMs: number;
    lastCheckedAgoMs: number | null;
    pollingFailed: boolean;
  }

  let {
    status,
    phase,
    elapsedMs,
    phaseElapsedMs,
    lastCheckedAgoMs,
    pollingFailed,
  }: Props = $props();

  const isTerminal = $derived(status === "completed" || status === "cancelled" || status === "failed");
  const isStale = $derived((lastCheckedAgoMs ?? elapsedMs) > 10_000);
  const animationPaused = $derived(status !== "running" || pollingFailed || isStale);
  const phaseIsKnown = $derived(meetingPhases.includes(phase as MeetingProgressPhase));
  const checkState = $derived(statusCheckState(lastCheckedAgoMs, pollingFailed));
  const accessibleProgressText = $derived(
    status === "completed"
      ? "Meeting processing complete"
      : status === "cancelled"
        ? "Meeting processing cancelled"
        : status === "failed"
          ? "Meeting processing failed"
          : status === "cancelling"
            ? "Cancellation requested; waiting for the worker"
            : "Meeting processing is in progress; completion percentage is unavailable",
  );
</script>

<section class="meeting-progress" aria-label="Meeting progress">
  <div class="progress-heading">
    <div>
      <h2>{statusTitle(status)}</h2>
      {#if !isTerminal}
        <p class="phase-label">{phaseLabel(phase)}</p>
      {/if}
    </div>
    <div class="elapsed">Elapsed: {formatDuration(elapsedMs)}</div>
  </div>

  <div
    class:paused={animationPaused}
    class:terminal={isTerminal}
    class:success={status === "completed"}
    class="progress-track"
    role="progressbar"
    aria-label="Meeting processing progress"
    aria-valuemin="0"
    aria-valuemax="100"
    aria-valuetext={accessibleProgressText}
  >
    <div class="progress-indicator"></div>
  </div>

  <div class="progress-meta">
    <span class="phase-time">This stage: {formatDuration(phaseElapsedMs)}</span>
    {#if status === "cancelling"}
      <span class="status-note">Cancellation requested — waiting for the worker.</span>
    {:else if status === "completed"}
      <span class="status-note">Completed successfully.</span>
    {:else if status === "cancelled"}
      <span class="status-note">The worker stopped before completion.</span>
    {:else if status === "failed"}
      <span class="status-note">The worker could not finish processing.</span>
    {:else if !isTerminal && phaseIsKnown}
      <span class="status-note">{phaseDetail(phase)}</span>
    {/if}
  </div>

  {#if !isTerminal}
    <p class:warning={pollingFailed || isStale} class="status-check">
      {statusCheckMessage(lastCheckedAgoMs, pollingFailed)}
    </p>
    <p class="status-check-announcement" aria-live="polite" aria-atomic="true">
      {statusCheckAnnouncement(checkState)}
    </p>
  {/if}
</section>

<style>
  .meeting-progress {
    width: 100%;
    box-sizing: border-box;
    padding: 14px 15px;
    color: var(--text, #f1f1f3);
    background: var(--bg-secondary, #1c1c20);
    border: 1px solid var(--border, #36363d);
    border-radius: 10px;
  }

  .progress-heading,
  .progress-meta {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
  }

  h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    line-height: 1.35;
  }

  .phase-label {
    margin: 2px 0 0;
    color: var(--text-muted, #a5a5ae);
    font-size: 12px;
    line-height: 1.4;
  }

  .elapsed,
  .phase-time {
    font-variant-numeric: tabular-nums;
  }

  .elapsed {
    flex: 0 0 auto;
    color: var(--text-muted, #a5a5ae);
    font-size: 12px;
  }

  .progress-track {
    position: relative;
    height: 7px;
    margin-top: 14px;
    overflow: hidden;
    background: var(--border, #36363d);
    border-radius: 999px;
  }

  .progress-indicator {
    width: 34%;
    height: 100%;
    border-radius: inherit;
    background: var(--accent, #86a8ff);
    transform: translateX(-100%);
    animation: meeting-progress-sweep 1.7s ease-in-out infinite;
  }

  .progress-track.paused .progress-indicator {
    transform: translateX(0);
    animation-play-state: paused;
  }

  .progress-track.terminal .progress-indicator {
    width: 0;
    animation: none;
    transform: none;
  }

  .progress-track.terminal.success .progress-indicator {
    width: 100%;
    background: var(--success, #82d996);
  }

  .progress-meta {
    flex-wrap: wrap;
    margin-top: 7px;
    color: var(--text-muted, #a5a5ae);
    font-size: 11px;
    line-height: 1.45;
  }

  .status-note {
    flex: 1 1 260px;
    text-align: right;
  }

  .status-check {
    margin: 9px 0 0;
    color: var(--text-muted, #a5a5ae);
    font-size: 11px;
    line-height: 1.4;
  }

  .status-check.warning {
    color: var(--warning, #e8bd72);
  }

  .status-check-announcement {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }

  @keyframes meeting-progress-sweep {
    from { transform: translateX(-100%); }
    to { transform: translateX(300%); }
  }

  @media (max-width: 420px) {
    .meeting-progress { padding: 12px; }
    .progress-meta { align-items: flex-start; flex-direction: column; gap: 3px; }
    .status-note { flex: none; text-align: left; }
  }

  @media (prefers-reduced-motion: reduce) {
    .progress-indicator { animation: none; transform: translateX(0); }
  }
</style>
