<script lang="ts">
  import { stageRows, type StageState } from "./transcribe-stages";
  let { state, cancelling = false }: { state: StageState; cancelling?: boolean } = $props();
  const rows = $derived(stageRows(state));
</script>

<ol class="transcription-stages" aria-label="Transcription steps">
  {#each rows as row, index (row.title)}
    <li class:done={row.status === "completed"} class:active={row.status === "active"}
      class:interrupted={row.status === "failed" || row.status === "cancelled"}
      aria-current={row.status === "active" ? "step" : undefined}>
      <span class="step-icon" aria-hidden="true">{row.status === "completed" ? "✓" : index + 1}</span>
      <div class="step-body">
        <div class="step-heading">
          <span class="step-title">{row.title}</span>
          <span class="step-status">{row.status === "active" && row.percent !== null ? `${row.percent}%` : row.status === "completed" ? "Done" : row.status === "pending" ? "Waiting" : ""}</span>
        </div>
        <div class="stage-meter" role="progressbar" aria-label={row.title}
          aria-valuemin="0" aria-valuemax="100" aria-valuenow={row.percent ?? undefined}
          aria-valuetext={cancelling && row.status === "active" ? "Cancelling…" : row.detail}>
          <div class="stage-fill" class:indeterminate={row.status === "active" && row.percent === null}
            style:width={`${row.percent ?? 30}%`}></div>
        </div>
        {#if row.status === "active" || row.status === "failed" || row.status === "cancelled"}
          <div class="step-detail" aria-live="polite">{cancelling && row.status === "active" ? "Cancelling…" : row.detail}</div>
        {/if}
      </div>
    </li>
  {/each}
</ol>

<style>
  .transcription-stages { width: 100%; margin: 0; padding: 0; list-style: none; text-align: left; }
  li { display: flex; gap: 9px; align-items: flex-start; padding: 6px 0; color: var(--text-muted); }
  .step-icon { display: grid; place-items: center; flex: 0 0 19px; height: 19px; border: 1px solid var(--border); border-radius: 5px; font-size: 11px; }
  .step-body { flex: 1; min-width: 0; }
  .step-heading { display: flex; justify-content: space-between; align-items: baseline; gap: 8px; font-size: 12px; line-height: 19px; }
  .step-title { font-weight: 500; }
  .step-status { font-size: 11px; flex-shrink: 0; font-variant-numeric: tabular-nums; }
  .stage-meter { height: 4px; overflow: hidden; background: var(--border); border-radius: 2px; margin-top: 4px; }
  .stage-fill { height: 100%; background: var(--accent); transition: width 150ms linear; }
  .active { color: var(--text); }
  .active .step-icon { border-color: var(--accent); color: var(--accent); }
  .done .step-icon { background: #82d996; color: #111113; border-color: #82d996; font-weight: 700; }
  .done .stage-fill { background: #82d996; }
  .interrupted .step-icon { border-color: var(--danger); color: var(--danger); }
  .interrupted .stage-fill { background: var(--danger); }
  .step-detail { margin-top: 3px; color: var(--text-muted); font-size: 11px; line-height: 1.4; }
  .indeterminate { animation: step-sweep 1.6s ease-in-out infinite alternate; }
  @keyframes step-sweep { from { transform: translateX(0); } to { transform: translateX(230%); } }
  @media (prefers-reduced-motion: reduce) { .indeterminate { animation: none; } .stage-fill { transition: none; } }
</style>
