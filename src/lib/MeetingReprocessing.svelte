<script lang="ts">
  import type { CorrectionOperation } from "./meeting-types";
  import { toErrorMessage } from "./errors";
  import type {
    ProposalState,
    RequiredWork,
    ReprocessingMode,
    ReprocessingResult,
    SelectedReprocessingPlan,
  } from "./meeting-reprocessing-types";
  import { proposalIsCurrent, proposalIsFullyApplied, proposalWasExported, selectedPlanIsCurrent } from "./meeting-reprocessing-state";
  import ProposalPanel from "./meeting-reprocessing/ProposalPanel.svelte";

  interface Props {
    idPrefix?: string;
    busy: boolean;
    draftDirty: boolean;
    currentReviewRevision?: string | null;
    selected: SelectedReprocessingPlan | null;
    proposal: ProposalState | null;
    result: ReprocessingResult | null;
    onPlan: (mode: ReprocessingMode, threshold: number, saveCache: boolean) => Promise<void>;
    onExecute: () => Promise<void>;
    onResolve: (index: number, operations: CorrectionOperation[]) => Promise<void>;
    onAccept: () => Promise<void>;
    onSave: () => Promise<boolean>;
    onOpen: () => Promise<void>;
    onDiscard: () => void;
  }

  let {
    idPrefix = "",
    busy,
    draftDirty,
    currentReviewRevision = null,
    selected,
    proposal,
    result,
    onPlan,
    onExecute,
    onResolve,
    onAccept,
    onSave,
    onOpen,
    onDiscard,
  }: Props = $props();

  let mode = $state<ReprocessingMode>("recluster");
  let threshold = $state(0.34);
  let saveCache = $state(false);
  let pendingAction = $state<string | null>(null);
  let error = $state("");
  let notice = $state("");
  let proposalRevision = $state<string | null>(null);
  let exportedProposalRevision = $state<string | null>(null);

  const modeDescriptions: Record<ReprocessingMode, { label: string; detail: string }> = {
    recluster: {
      label: "Recluster",
      detail: "Reuse transcription, language detection, segmentation, and embeddings; recompute clustering only.",
    },
    rediarize: {
      label: "Re-diarize",
      detail: "Reuse transcription and language detection; decode audio and recompute segmentation, embeddings, and clustering.",
    },
    full: {
      label: "Full recomputation",
      detail: "Recompute audio decode, transcription, language detection, segmentation, embeddings, and clustering.",
    },
  };

  const requiredWorkLabels: Array<[keyof RequiredWork, string]> = [
    ["decode_audio", "Decode audio"],
    ["transcription", "Transcription"],
    ["language_detection", "Language detection"],
    ["segmentation", "Segmentation"],
    ["embeddings", "Speaker embeddings"],
    ["clustering", "Speaker clustering"],
  ];

  $effect(() => {
    const revision = proposal?.proposal.revision ?? null;
    if (revision === proposalRevision) return;
    proposalRevision = revision;
    exportedProposalRevision = null;
    error = "";
  });

  function isBusy(): boolean {
    return busy || pendingAction !== null;
  }

  async function runAction(name: string, action: () => Promise<boolean | void>): Promise<boolean> {
    if (isBusy()) return false;
    error = "";
    notice = "";
    pendingAction = name;
    try {
      return (await action()) !== false;
    } catch (failure) {
      error = toErrorMessage(failure, "The requested meeting action failed. Your current review remains available.");
      return false;
    } finally {
      pendingAction = null;
    }
  }

  function selectMode(nextMode: ReprocessingMode): void {
    mode = nextMode;
    if (nextMode !== "full") saveCache = false;
  }

  function updateThreshold(event: Event): void {
    const value = Number((event.currentTarget as HTMLInputElement).value);
    if (Number.isFinite(value)) threshold = Math.min(2, Math.max(0, value));
  }

  async function plan(): Promise<void> {
    if (draftDirty) return;
    await runAction("plan", () => onPlan(mode, threshold, mode === "full" && saveCache));
  }

  async function execute(): Promise<void> {
    if (draftDirty || !selected || !selectedPlanIsCurrent(selected, currentReviewRevision)) return;
    await runAction("execute", onExecute);
  }

  async function accept(): Promise<void> {
    if (draftDirty || !canAccept()) return;
    if (
      !proposalWasExported(proposal, exportedProposalRevision) &&
      !window.confirm(
        "Accepting this proposal replaces the current review. Save the proposal first if you may need to recover it. Continue?",
      )
    ) {
      return;
    }
    await runAction("accept", onAccept);
  }

  async function save(): Promise<void> {
    const revision = proposal?.proposal.revision ?? null;
    const saved = await runAction("save", onSave);
    if (saved && revision !== null && proposal?.proposal.revision === revision) {
      exportedProposalRevision = revision;
      notice = "Reprocessing proposal saved.";
    }
  }

  async function open(): Promise<void> {
    if (proposal && !window.confirm("Open a saved proposal and discard the current proposal resolutions?")) return;
    await runAction("open", onOpen);
  }

  function discard(): void {
    if (isBusy()) return;
    if (proposal && !window.confirm("Discard this reprocessing proposal and all of its resolutions?")) return;
    error = "";
    notice = "";
    exportedProposalRevision = null;
    onDiscard();
  }

  function allApplied(): boolean {
    return proposalIsFullyApplied(proposal);
  }

  function canAccept(): boolean {
    return Boolean(proposal && proposalIsCurrent(proposal, currentReviewRevision) && allApplied() && !draftDirty);
  }

  function selectedPlanIsFresh(): boolean {
    return selectedPlanIsCurrent(selected, currentReviewRevision);
  }

  function proposalIsFresh(): boolean {
    return proposalIsCurrent(proposal, currentReviewRevision);
  }

  function proposalWasSaved(): boolean {
    return proposalWasExported(proposal, exportedProposalRevision);
  }

  function workItems(work: RequiredWork): Array<{ label: string; required: boolean }> {
    return requiredWorkLabels.map(([key, label]) => ({ label, required: work[key] }));
  }

  function timingItems(): Array<[string, number]> {
    if (!result) return [];
    return [
      ["Validation", result.timings.validation_seconds],
      ["Decode", result.timings.decode_seconds],
      ["Analysis", result.timings.analysis_seconds],
      ["Clustering", result.timings.clustering_seconds],
      ["Full pipeline", result.timings.full_pipeline_seconds],
      ["Proposal", result.timings.proposal_seconds],
      ["Total", result.timings.total_seconds],
    ];
  }
</script>

<section class="meeting-reprocessing" aria-labelledby={idPrefix + "meeting-reprocessing-title"}>
  <header class="header">
    <div>
      <p class="eyebrow">Meeting reprocessing</p>
      <h1 id={idPrefix + "meeting-reprocessing-title"}>Reprocess a meeting</h1>
      <p class="intro">
        Choose the smallest explicit recomputation, review every correction migration, then accept it into the current review.
      </p>
    </div>
    <div class="header-actions">
      <button type="button" class="secondary" disabled={isBusy()} onclick={() => void open()}>
        {pendingAction === "open" ? "Opening…" : "Open saved proposal"}
      </button>
    </div>
  </header>

  {#if error}
    <div class="error" role="alert"><strong>Reprocessing notice</strong><span>{error}</span></div>
  {/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}

  <section class="panel plan-panel" aria-labelledby={idPrefix + "plan-title"}>
    <div class="section-heading">
      <div>
        <h2 id={idPrefix + "plan-title"}>1. Plan recomputation</h2>
        <p>Planning validates the selected source and immutable context before any work runs. These controls set the next plan.</p>
      </div>
    </div>
    <div class="mode-list" role="radiogroup" aria-label="Reprocessing mode">
      {#each Object.entries(modeDescriptions) as [value, description] (value)}
        {@const option = value as ReprocessingMode}
        <label class:chosen={mode === option} class="mode-card">
          <input
            type="radio"
            name={idPrefix + "reprocessing-mode"}
            value={option}
            checked={mode === option}
            disabled={isBusy()}
            onchange={() => selectMode(option)}
          />
          <span>
            <strong>{description.label}</strong>
            <small>{description.detail}</small>
          </span>
        </label>
      {/each}
    </div>
    <div class="plan-options">
      <label class="field">
        <span>Speaker threshold</span>
        <input type="number" min="0" max="2" step="0.01" value={threshold} disabled={isBusy()} oninput={updateThreshold} />
        <small>Must remain between 0 and 2.</small>
      </label>
      {#if mode === "full"}
        <label class="cache-option">
          <input type="checkbox" bind:checked={saveCache} disabled={isBusy()} />
          <span>
            <strong>Save a new analysis cache</strong>
            <small
              >Optional. Cache output stores voice/transcript-derived data locally; manually delete the artifact when no longer needed.</small
            >
          </span>
        </label>
      {/if}
    </div>
    {#if selected && !selectedPlanIsFresh()}
      <p class="stale-warning" role="alert">
        This plan was made for an older review revision. Plan again before executing; the current review remains unchanged.
      </p>
    {/if}
    {#if draftDirty}
      <p class="draft-warning" role="status">
        Apply or discard the unsaved edits in Meeting review before planning, executing, resolving, or accepting reprocessing.
      </p>
    {/if}
    {#if selected && (mode !== selected.plan.mode || threshold !== selected.plan.threshold)}
      <p class="selected-plan-note" role="status">
        Execution uses the selected plan below ({modeDescriptions[selected.plan.mode].label}, threshold {selected.plan.threshold.toFixed(
          2,
        )}). Plan reprocessing to replace it with these settings.
      </p>
    {/if}
    <button type="button" class="primary" disabled={isBusy() || draftDirty} onclick={() => void plan()}>
      {pendingAction === "plan" ? "Planning…" : "Plan reprocessing"}
    </button>
  </section>

  {#if selected}
    <section class="panel selected-panel" aria-labelledby={idPrefix + "selected-plan-title"}>
      <div class="section-heading">
        <div>
          <h2 id={idPrefix + "selected-plan-title"}>Selected plan</h2>
          <p>{selected.file_path}</p>
        </div>
        <span class="status-pill">{modeDescriptions[selected.plan.mode].label}</span>
      </div>
      <p class="selected-plan-note">
        This is the plan that Execute will run. Mode: <strong>{modeDescriptions[selected.plan.mode].label}</strong>; threshold:
        <strong>{selected.plan.threshold.toFixed(2)}</strong>.
      </p>
      <dl class="context-list">
        <div>
          <dt>Mode</dt>
          <dd>{modeDescriptions[selected.plan.mode].label}</dd>
        </div>
        <div>
          <dt>Speaker threshold</dt>
          <dd>{selected.plan.threshold.toFixed(2)}</dd>
        </div>
        <div>
          <dt>Plan revision</dt>
          <dd>{selected.plan.revision}</dd>
        </div>
        <div>
          <dt>Previous revision</dt>
          <dd>{selected.plan.context.previous_revision}</dd>
        </div>
        {#if selected.cache_path}<div>
            <dt>Input cache</dt>
            <dd>{selected.cache_path}</dd>
          </div>{/if}
        {#if selected.cache_output}<div>
            <dt>New cache output</dt>
            <dd>{selected.cache_output}</dd>
          </div>{/if}
      </dl>
      <div class="work-grid" aria-label="Required work">
        {#each workItems(selected.plan.required_work) as item (item.label)}
          <span class:required={item.required} class="work-item">{item.required ? "Run" : "Skip"}: {item.label}</span>
        {/each}
      </div>
      <button type="button" class="primary" disabled={isBusy() || draftDirty || !selectedPlanIsFresh()} onclick={() => void execute()}>
        {pendingAction === "execute" ? "Running…" : `Execute selected ${modeDescriptions[selected.plan.mode].label} plan`}
      </button>
    </section>
  {/if}

  {#if proposal}
    <ProposalPanel
      {idPrefix}
      {proposal}
      {pendingAction}
      {draftDirty}
      {isBusy}
      {runAction}
      {onResolve}
      {discard}
      {save}
      {accept}
      {proposalIsFresh}
      {proposalWasSaved}
      {allApplied}
      {canAccept}
    />
  {/if}

  {#if result}
    <section class="panel result-panel" aria-labelledby={idPrefix + "result-title"}>
      <div class="section-heading">
        <div>
          <h2 id={idPrefix + "result-title"}>Reprocessing result</h2>
          <p>Measured work and timing from the completed operation.</p>
        </div>
      </div>
      <div class="work-grid" aria-label="Completed work">
        {#each workItems(result.required_work) as item (item.label)}<span class:required={item.required} class="work-item"
            >{item.required ? "Ran" : "Skipped"}: {item.label}</span
          >{/each}
      </div>
      <dl class="timing-grid">
        {#each timingItems() as [label, seconds] (label)}<div>
            <dt>{label}</dt>
            <dd>{seconds.toFixed(2)}s</dd>
          </div>{/each}
      </dl>
    </section>
  {/if}
</section>

<style>
  .meeting-reprocessing {
    width: min(100%, 960px);
    margin: 0 auto;
    padding: 24px clamp(16px, 4vw, 40px) 48px;
    overflow-wrap: anywhere;
  }
  .header,
  .section-heading,
  .plan-options {
    display: flex;
    gap: 16px;
    align-items: flex-start;
    justify-content: space-between;
  }
  .header {
    padding-bottom: 24px;
    border-bottom: 1px solid var(--border);
  }
  .eyebrow {
    color: var(--accent);
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
  h1 {
    margin-top: 2px;
    font-size: 24px;
  }
  h2 {
    font-size: 16px;
  }
  .intro,
  .section-heading p,
  .field small,
  .cache-option small,
  .mode-card small,
  .draft-warning {
    color: var(--text-muted);
    font-size: 12px;
  }
  .intro {
    max-width: 680px;
    margin-top: 6px;
  }
  .panel {
    margin-top: 24px;
    padding: 18px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .section-heading {
    margin-bottom: 14px;
  }
  .section-heading p {
    margin-top: 4px;
  }
  .mode-list {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 8px;
  }
  .mode-card {
    display: flex;
    gap: 9px;
    align-items: flex-start;
    min-height: 92px;
    padding: 12px;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
  }
  .mode-card.chosen {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .mode-card span {
    display: grid;
    gap: 5px;
  }
  .mode-card strong {
    font-size: 14px;
  }
  .plan-options {
    justify-content: flex-start;
    flex-wrap: wrap;
    margin: 16px 0;
  }
  .field {
    display: grid;
    gap: 5px;
    min-width: 160px;
  }
  .field > span,
  .cache-option strong {
    color: var(--text);
    font-size: 12px;
    font-weight: 600;
  }
  input[type="number"] {
    box-sizing: border-box;
    min-height: 36px;
    padding: 7px 9px;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    font: inherit;
  }
  input:focus-visible,
  button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .cache-option {
    display: flex;
    gap: 8px;
    max-width: 560px;
    align-items: flex-start;
    padding: 9px 0;
  }
  .cache-option span {
    display: grid;
    gap: 4px;
  }
  .draft-warning,
  .selected-plan-note,
  .stale-warning {
    margin: 12px 0;
    padding: 10px 12px;
    border-left: 3px solid var(--accent);
    background: rgba(255, 255, 255, 0.04);
  }
  .stale-warning {
    color: var(--danger);
    border-left-color: var(--danger);
  }
  button {
    min-height: 36px;
    padding: 8px 12px;
    border-radius: var(--radius);
    font: inherit;
    cursor: pointer;
  }
  button:disabled {
    cursor: default;
    opacity: 0.55;
  }
  .primary {
    color: var(--bg);
    background: var(--accent);
    border: 1px solid var(--accent);
    font-weight: 700;
  }
  .secondary {
    color: var(--text);
    background: transparent;
    border: 1px solid var(--border);
  }
  .error {
    display: grid;
    gap: 5px;
    margin-top: 16px;
    padding: 12px;
    color: var(--text);
    background: rgba(255, 107, 107, 0.1);
    border: 1px solid var(--danger);
    border-radius: var(--radius);
  }
  .error span {
    color: #ffb0b0;
  }
  .notice {
    margin-top: 12px;
    color: var(--accent);
    font-size: 13px;
  }
  .header-actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    flex-shrink: 0;
  }
  .header-actions button {
    flex: 0 0 auto;
    overflow-wrap: normal;
  }
  .status-pill,
  .work-item {
    color: var(--text-muted);
    font-size: 11px;
  }
  .status-pill {
    padding: 5px 8px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 999px;
    white-space: nowrap;
  }
  .context-list,
  .timing-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
    margin: 0 0 14px;
  }
  .context-list div,
  .timing-grid div {
    min-width: 0;
    padding: 9px;
    background: var(--bg);
    border-radius: var(--radius);
  }
  dt {
    color: var(--text-muted);
    font-size: 11px;
  }
  dd {
    margin-top: 3px;
    color: var(--text);
    font-size: 12px;
    overflow-wrap: anywhere;
  }
  .work-grid {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 7px;
    margin: 12px 0 16px;
  }
  .work-item {
    padding: 8px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .work-item.required {
    color: var(--accent);
    border-color: var(--accent);
  }
  .timing-grid {
    grid-template-columns: repeat(4, minmax(0, 1fr));
  }
  @media (max-width: 700px) {
    .header,
    .section-heading {
      flex-direction: column;
    }
    .mode-list,
    .work-grid,
    .timing-grid,
    .context-list {
      grid-template-columns: 1fr;
    }
    .header-actions {
      width: 100%;
    }
    .header-actions button {
      flex: 1;
    }
  }
</style>
