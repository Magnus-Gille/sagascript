<script lang="ts">
  import type { CorrectionOperation, MeetingSegment, MeetingSpeaker } from "./meeting-types";
  import type {
    MigrationStep,
    ProposalState,
    RequiredWork,
    ReprocessingMode,
    ReprocessingResult,
    SelectedReprocessingPlan,
  } from "./meeting-reprocessing-types";
  import {
    filterCandidateSegments,
    proposalIsCurrent,
    proposalIsFullyApplied,
    proposalWasExported,
    selectedPlanIsCurrent,
  } from "./meeting-reprocessing-state.js";

  interface Props {
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

  type ResolutionOperation = CorrectionOperation;

  let {
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
  let threshold = $state(0.75);
  let saveCache = $state(false);
  let pendingAction = $state<string | null>(null);
  let error = $state("");
  let notice = $state("");
  let proposalRevision = $state<string | null>(null);
  let exportedProposalRevision = $state<string | null>(null);
  let candidateSegmentQuery = $state("");
  let resolutionDrafts = $state<Record<number, ResolutionOperation[]>>({});

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
    candidateSegmentQuery = "";
    resolutionDrafts = {};
    error = "";
  });

  function isBusy(): boolean {
    return busy || pendingAction !== null;
  }

  function errorText(value: unknown): string {
    if (typeof value === "string" && value.trim()) return value;
    if (value instanceof Error && value.message) return value.message;
    return "The requested meeting action failed. Your current review remains available.";
  }

  async function runAction(name: string, action: () => Promise<boolean | void>): Promise<boolean> {
    if (isBusy()) return false;
    error = "";
    notice = "";
    pendingAction = name;
    try {
      return (await action()) !== false;
    } catch (failure) {
      error = errorText(failure);
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
    if (!proposalWasExported(proposal, exportedProposalRevision)
      && !window.confirm("Accepting this proposal replaces the current review. Save the proposal first if you may need to recover it. Continue?")) {
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

  function candidateSegments(): MeetingSegment[] {
    return proposal?.candidate.segments ?? [];
  }

  function candidateSpeakers(): MeetingSpeaker[] {
    return proposal?.candidate.speakers ?? [];
  }

  function sourceSegment(operation: CorrectionOperation): MeetingSegment | undefined {
    if (operation.kind !== "edit_segment" || !proposal) return undefined;
    return proposal.proposal.previous.original.segments.find((segment) => segment.id === operation.segment_id);
  }

  function candidateSegment(id: string): MeetingSegment | undefined {
    return candidateSegments().find((segment) => segment.id === id);
  }

  function filteredCandidateSegments(selectedId: string): MeetingSegment[] {
    return filterCandidateSegments(candidateSegments(), candidateSegmentQuery, selectedId ? [selectedId] : []);
  }

  function formatSeconds(seconds: number): string {
    const safeSeconds = Number.isFinite(seconds) ? Math.max(0, seconds) : 0;
    const minutes = Math.floor(safeSeconds / 60);
    const remainder = (safeSeconds % 60).toFixed(2).padStart(5, "0");
    return `${minutes}:${remainder}`;
  }

  function operationSummary(operation: CorrectionOperation): string {
    if (operation.kind === "edit_segment") {
      const changes: string[] = [];
      if (operation.text !== undefined) changes.push(`text → “${operation.text}”`);
      if (operation.speaker_id !== undefined) changes.push(`speaker → ${operation.speaker_id}`);
      return `Edit segment ${operation.segment_id || "(target not selected)"}: ${changes.join(", ") || "no fields selected"}`;
    }
    if (operation.kind === "rename_speaker") {
      return `Rename ${operation.speaker_id || "(speaker not selected)"} → “${operation.label}”`;
    }
    return `Merge ${operation.from_id || "(source not selected)"} → ${operation.into_id || "(target not selected)"}`;
  }

  function emptyResolution(operation: CorrectionOperation): ResolutionOperation {
    if (operation.kind === "edit_segment") {
      return {
        kind: "edit_segment",
        segment_id: "",
        ...(operation.text !== undefined ? { text: operation.text } : {}),
        ...(operation.speaker_id !== undefined ? { speaker_id: operation.speaker_id } : {}),
      };
    }
    if (operation.kind === "rename_speaker") {
      return { kind: "rename_speaker", speaker_id: "", label: operation.label };
    }
    return { kind: "merge_speakers", from_id: "", into_id: "" };
  }

  function resolutionRows(index: number, original: CorrectionOperation): ResolutionOperation[] {
    return resolutionDrafts[index] ?? [emptyResolution(original)];
  }

  function updateResolution(index: number, rowIndex: number, operation: ResolutionOperation): void {
    const rows = [...(resolutionDrafts[index] ?? [])];
    rows[rowIndex] = operation;
    resolutionDrafts = { ...resolutionDrafts, [index]: rows };
  }

  function addTarget(index: number, original: CorrectionOperation): void {
    if (original.kind !== "edit_segment") return;
    const rows = resolutionRows(index, original);
    rows.push(emptyResolution(original));
    resolutionDrafts = { ...resolutionDrafts, [index]: rows };
  }

  function removeTarget(index: number, rowIndex: number, original: CorrectionOperation): void {
    const rows = resolutionRows(index, original).filter((_, currentIndex) => currentIndex !== rowIndex);
    resolutionDrafts = { ...resolutionDrafts, [index]: rows };
  }

  function validResolution(index: number, original: CorrectionOperation): boolean {
    const rows = resolutionRows(index, original);
    if (rows.length === 0) return false;
    const speakerIds = new Set(candidateSpeakers().map((speaker) => speaker.id));
    const segmentIds = new Set(candidateSegments().map((segment) => segment.id));
    if (original.kind === "edit_segment") {
      const targetIds = rows.map((row) => row.kind === "edit_segment" ? row.segment_id : "");
      return rows.every((row) =>
        row.kind === "edit_segment"
        && segmentIds.has(row.segment_id)
        && (row.text !== undefined || row.speaker_id !== undefined)
        && (row.speaker_id === undefined || speakerIds.has(row.speaker_id))
      ) && new Set(targetIds).size === targetIds.length;
    }
    if (original.kind === "rename_speaker") {
      const row = rows[0];
      return row.kind === "rename_speaker" && speakerIds.has(row.speaker_id) && row.label.trim().length > 0;
    }
    const row = rows[0];
    return row.kind === "merge_speakers"
      && speakerIds.has(row.from_id)
      && speakerIds.has(row.into_id)
      && row.from_id !== row.into_id;
  }

  async function resolve(index: number, original: CorrectionOperation): Promise<void> {
    if (draftDirty || !validResolution(index, original)) return;
    await runAction(`resolve-${index}`, () => onResolve(index, resolutionRows(index, original)));
  }

  function firstConflictIndex(): number {
    return proposal?.preview.steps.findIndex((step) => step.status === "conflict") ?? -1;
  }

  function allApplied(): boolean {
    return proposalIsFullyApplied(proposal);
  }

  function canAccept(): boolean {
    return Boolean(
      proposal
      && proposalIsCurrent(proposal, currentReviewRevision)
      && allApplied()
      && !draftDirty,
    );
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

<section class="meeting-reprocessing" aria-labelledby="meeting-reprocessing-title">
  <header class="header">
    <div>
      <p class="eyebrow">Meeting reprocessing</p>
      <h1 id="meeting-reprocessing-title">Reprocess a meeting</h1>
      <p class="intro">Choose the smallest explicit recomputation, review every correction migration, then accept it into the current review.</p>
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

  <section class="panel plan-panel" aria-labelledby="plan-title">
    <div class="section-heading">
      <div>
        <h2 id="plan-title">1. Plan recomputation</h2>
        <p>Planning validates the selected source and immutable context before any work runs. These controls set the next plan.</p>
      </div>
    </div>
    <div class="mode-list" role="radiogroup" aria-label="Reprocessing mode">
      {#each Object.entries(modeDescriptions) as [value, description]}
        {@const option = value as ReprocessingMode}
        <label class:chosen={mode === option} class="mode-card">
          <input type="radio" name="reprocessing-mode" value={option} checked={mode === option} disabled={isBusy()} onchange={() => selectMode(option)} />
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
            <small>Optional. Cache output stores voice/transcript-derived data locally; manually delete the artifact when no longer needed.</small>
          </span>
        </label>
      {/if}
    </div>
    {#if selected && !selectedPlanIsFresh()}
      <p class="stale-warning" role="alert">This plan was made for an older review revision. Plan again before executing; the current review remains unchanged.</p>
    {/if}
    {#if draftDirty}
      <p class="draft-warning" role="status">Apply or discard the unsaved edits in Meeting review before planning, executing, resolving, or accepting reprocessing.</p>
    {/if}
    {#if selected && (mode !== selected.plan.mode || threshold !== selected.plan.threshold)}
      <p class="selected-plan-note" role="status">
        Execution uses the selected plan below ({modeDescriptions[selected.plan.mode].label}, threshold {selected.plan.threshold.toFixed(2)}). Plan reprocessing to replace it with these settings.
      </p>
    {/if}
    <button type="button" class="primary" disabled={isBusy() || draftDirty} onclick={() => void plan()}>
      {pendingAction === "plan" ? "Planning…" : "Plan reprocessing"}
    </button>
  </section>

  {#if selected}
    <section class="panel selected-panel" aria-labelledby="selected-plan-title">
      <div class="section-heading">
        <div>
          <h2 id="selected-plan-title">Selected plan</h2>
          <p>{selected.file_path}</p>
        </div>
        <span class="status-pill">{modeDescriptions[selected.plan.mode].label}</span>
      </div>
      <p class="selected-plan-note">This is the plan that Execute will run. Mode: <strong>{modeDescriptions[selected.plan.mode].label}</strong>; threshold: <strong>{selected.plan.threshold.toFixed(2)}</strong>.</p>
      <dl class="context-list">
        <div><dt>Mode</dt><dd>{modeDescriptions[selected.plan.mode].label}</dd></div>
        <div><dt>Speaker threshold</dt><dd>{selected.plan.threshold.toFixed(2)}</dd></div>
        <div><dt>Plan revision</dt><dd>{selected.plan.revision}</dd></div>
        <div><dt>Previous revision</dt><dd>{selected.plan.context.previous_revision}</dd></div>
        {#if selected.cache_path}<div><dt>Input cache</dt><dd>{selected.cache_path}</dd></div>{/if}
        {#if selected.cache_output}<div><dt>New cache output</dt><dd>{selected.cache_output}</dd></div>{/if}
      </dl>
      <div class="work-grid" aria-label="Required work">
        {#each workItems(selected.plan.required_work) as item}
          <span class:required={item.required} class="work-item">{item.required ? "Run" : "Skip"}: {item.label}</span>
        {/each}
      </div>
      <button type="button" class="primary" disabled={isBusy() || draftDirty || !selectedPlanIsFresh()} onclick={() => void execute()}>
        {pendingAction === "execute" ? "Running…" : `Execute selected ${modeDescriptions[selected.plan.mode].label} plan`}
      </button>
    </section>
  {/if}

  {#if proposal}
    <section class="panel proposal-panel" aria-labelledby="proposal-title">
      <div class="section-heading">
        <div>
          <h2 id="proposal-title">2. Review migration proposal</h2>
          <p>{proposal.preview.steps.length} correction step{proposal.preview.steps.length === 1 ? "" : "s"}. Automatic mappings stay read-only; conflicts require an explicit target.</p>
        </div>
        <div class="proposal-actions">
          <button type="button" class="secondary" disabled={isBusy()} onclick={discard}>Discard proposal</button>
          <button type="button" class="secondary" disabled={isBusy()} onclick={() => void save()}>{pendingAction === "save" ? "Saving…" : "Save proposal"}</button>
        </div>
      </div>
      {#if !proposalIsFresh()}
        <p class="stale-warning" role="alert">This proposal was made from an older review revision. Save/export remains available, but accepting is disabled; generate a new proposal for the current review.</p>
      {:else if !proposalWasSaved()}
        <p class="proposal-save-warning" role="status">Save/export this proposal before accepting if you may need to recover the migration.</p>
      {/if}

      {#if proposal.preview.steps.length === 0}
        <p class="empty">No previous correction operations need migration.</p>
      {:else}
        <div class="proposal-list">
          {#each proposal.preview.steps as step, index (index)}
            {@const source = sourceSegment(step.original)}
            {@const activeConflict = step.status === "conflict" && index === firstConflictIndex()}
            <article class:active-conflict={activeConflict} class="proposal-step">
              <div class="step-heading">
                <span class="step-number">{index + 1}</span>
                <div>
                  <strong>{step.status === "applied" ? "Applied automatically" : step.status === "conflict" ? "Conflict needs a decision" : "Blocked until the earlier conflict is resolved"}</strong>
                  <p class="operation-summary">Original: {operationSummary(step.original)}</p>
                </div>
              </div>

              {#if source}
                <div class="comparison" aria-label="Original and candidate segment comparison">
                  <div><span>Previous original · {formatSeconds(source.start)}–{formatSeconds(source.end)}</span><p>{source.text || "(empty text)"}</p></div>
                  <div><span>Candidate transcript</span><p>Select an explicit target below; candidate text and timing will appear here.</p></div>
                </div>
              {/if}

              {#if step.status === "applied"}
                <div class="mapped-list">
                  <span class="subheading">Mapped disposition</span>
                  {#each step.mapped ?? [] as mapped}<p>{operationSummary(mapped)}</p>{/each}
                </div>
              {:else if activeConflict}
                <div class="resolution" aria-label={`Resolve conflict ${index + 1}`}>
                  <span class="subheading">Choose candidate target{step.original.kind === "edit_segment" ? "s" : ""}</span>
                  {#if step.original.kind === "edit_segment"}
                    <label class="field wide candidate-search">
                      <span>Find candidate segments</span>
                      <input type="text" value={candidateSegmentQuery} placeholder="Search text, id, or time (for example 12:34)" disabled={isBusy() || draftDirty} oninput={(event) => { candidateSegmentQuery = (event.currentTarget as HTMLInputElement).value; }} />
                      <small>{filterCandidateSegments(candidateSegments(), candidateSegmentQuery).length} matching candidate{filterCandidateSegments(candidateSegments(), candidateSegmentQuery).length === 1 ? "" : "s"}; selected targets remain visible.</small>
                    </label>
                  {/if}
                  {#each resolutionRows(index, step.original) as operation, rowIndex (rowIndex)}
                    {#if operation.kind === "edit_segment"}
                      <div class="resolution-row">
                        <label class="field wide">
                          <span>Candidate segment</span>
                          <select
                            value={operation.segment_id}
                            onchange={(event) => updateResolution(index, rowIndex, { ...operation, segment_id: (event.currentTarget as HTMLSelectElement).value })}
                          >
                            <option value="">Choose a candidate segment</option>
                            {#each filteredCandidateSegments(operation.segment_id) as segment (segment.id)}
                              <option value={segment.id}>{formatSeconds(segment.start)}–{formatSeconds(segment.end)} · {segment.text || "(empty text)"}</option>
                            {/each}
                          </select>
                        </label>
                        <label class="field wide">
                          <span>Replacement text {operation.text === undefined ? "(unchanged)" : ""}</span>
                          <input type="text" value={operation.text ?? ""} oninput={(event) => updateResolution(index, rowIndex, { ...operation, text: (event.currentTarget as HTMLInputElement).value })} />
                        </label>
                        <label class="field wide">
                          <span>Speaker {operation.speaker_id === undefined ? "(unchanged)" : ""}</span>
                          <select
                            value={operation.speaker_id ?? ""}
                            onchange={(event) => {
                              const speakerId = (event.currentTarget as HTMLSelectElement).value;
                              updateResolution(index, rowIndex, speakerId ? { ...operation, speaker_id: speakerId } : { kind: "edit_segment", segment_id: operation.segment_id, ...(operation.text !== undefined ? { text: operation.text } : {}) });
                            }}
                          >
                            <option value="">Keep candidate speaker</option>
                            {#each candidateSpeakers() as speaker (speaker.id)}<option value={speaker.id}>{speaker.label} ({speaker.id})</option>{/each}
                          </select>
                        </label>
                        {#if candidateSegment(operation.segment_id)}
                          {@const target = candidateSegment(operation.segment_id)}
                          <div class="candidate-detail"><span>Candidate · {formatSeconds(target!.start)}–{formatSeconds(target!.end)}</span><p>{target!.text || "(empty text)"}</p></div>
                        {/if}
                        {#if resolutionRows(index, step.original).length > 1}
                          <button type="button" class="text-button" onclick={() => removeTarget(index, rowIndex, step.original)}>Remove target</button>
                        {/if}
                      </div>
                    {:else if operation.kind === "rename_speaker"}
                      <div class="resolution-row two-column">
                        <label class="field">
                          <span>Candidate speaker</span>
                          <select value={operation.speaker_id} onchange={(event) => updateResolution(index, rowIndex, { ...operation, speaker_id: (event.currentTarget as HTMLSelectElement).value })}>
                            <option value="">Choose a candidate speaker</option>
                            {#each candidateSpeakers() as speaker (speaker.id)}<option value={speaker.id}>{speaker.label} ({speaker.id})</option>{/each}
                          </select>
                        </label>
                        <label class="field"><span>New label</span><input type="text" value={operation.label} oninput={(event) => updateResolution(index, rowIndex, { ...operation, label: (event.currentTarget as HTMLInputElement).value })} /></label>
                      </div>
                    {:else}
                      <div class="resolution-row two-column">
                        <label class="field">
                          <span>Speaker to merge</span>
                          <select value={operation.from_id} onchange={(event) => updateResolution(index, rowIndex, { ...operation, from_id: (event.currentTarget as HTMLSelectElement).value })}>
                            <option value="">Choose source speaker</option>
                            {#each candidateSpeakers() as speaker (speaker.id)}<option value={speaker.id}>{speaker.label} ({speaker.id})</option>{/each}
                          </select>
                        </label>
                        <label class="field">
                          <span>Merge into</span>
                          <select value={operation.into_id} onchange={(event) => updateResolution(index, rowIndex, { ...operation, into_id: (event.currentTarget as HTMLSelectElement).value })}>
                            <option value="">Choose target speaker</option>
                            {#each candidateSpeakers() as speaker (speaker.id)}<option value={speaker.id}>{speaker.label} ({speaker.id})</option>{/each}
                          </select>
                        </label>
                      </div>
                    {/if}
                  {/each}
                  {#if step.original.kind === "edit_segment"}
                    <button type="button" class="secondary" disabled={isBusy()} onclick={() => addTarget(index, step.original)}>Add another candidate target</button>
                  {/if}
                  {#if !validResolution(index, step.original)}<p class="resolution-warning">Choose at least one valid candidate target. Empty resolutions are not accepted.</p>{/if}
                  <button type="button" class="primary" disabled={isBusy() || draftDirty || !validResolution(index, step.original)} onclick={() => void resolve(index, step.original)}>
                    {pendingAction === `resolve-${index}` ? "Resolving…" : "Apply this resolution"}
                  </button>
                </div>
              {:else}
                <p class="blocked-note">Resolve the first conflict before this step can be changed. No correction is silently dropped.</p>
              {/if}
            </article>
          {/each}
        </div>
      {/if}

      <div class="accept-row">
        <p>{allApplied() ? "Every correction migration is applied." : "All conflicts must be resolved before accepting this proposal."}</p>
        <button type="button" class="primary" disabled={isBusy() || draftDirty || !canAccept()} onclick={() => void accept()}>
          {pendingAction === "accept" ? "Accepting…" : "Accept proposal"}
        </button>
      </div>
    </section>
  {/if}

  {#if result}
    <section class="panel result-panel" aria-labelledby="result-title">
      <div class="section-heading"><div><h2 id="result-title">Reprocessing result</h2><p>Measured work and timing from the completed operation.</p></div></div>
      <div class="work-grid" aria-label="Completed work">
        {#each workItems(result.required_work) as item}<span class:required={item.required} class="work-item">{item.required ? "Ran" : "Skipped"}: {item.label}</span>{/each}
      </div>
      <dl class="timing-grid">
        {#each timingItems() as [label, seconds]}<div><dt>{label}</dt><dd>{seconds.toFixed(2)}s</dd></div>{/each}
      </dl>
    </section>
  {/if}
</section>

<style>
  .meeting-reprocessing { width: min(100%, 960px); margin: 0 auto; padding: 24px clamp(16px, 4vw, 40px) 48px; overflow-wrap: anywhere; }
  .header, .section-heading, .accept-row, .plan-options { display: flex; gap: 16px; align-items: flex-start; justify-content: space-between; }
  .header { padding-bottom: 24px; border-bottom: 1px solid var(--border); }
  .eyebrow { color: var(--accent); font-size: 11px; font-weight: 700; letter-spacing: .08em; text-transform: uppercase; }
  h1 { margin-top: 2px; font-size: 24px; } h2 { font-size: 16px; }
  .intro, .section-heading p, .field small, .cache-option small, .mode-card small, .draft-warning, .blocked-note, .resolution-warning, .accept-row p { color: var(--text-muted); font-size: 12px; }
  .intro { max-width: 680px; margin-top: 6px; }
  .panel { margin-top: 24px; padding: 18px; background: var(--bg-secondary); border: 1px solid var(--border); border-radius: var(--radius); }
  .section-heading { margin-bottom: 14px; } .section-heading p { margin-top: 4px; }
  .mode-list { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; }
  .mode-card { display: flex; gap: 9px; align-items: flex-start; min-height: 92px; padding: 12px; color: var(--text); background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius); cursor: pointer; }
  .mode-card.chosen { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); } .mode-card span { display: grid; gap: 5px; } .mode-card strong { font-size: 14px; }
  .plan-options { justify-content: flex-start; flex-wrap: wrap; margin: 16px 0; }
  .field { display: grid; gap: 5px; min-width: 160px; } .field > span, .cache-option strong { color: var(--text); font-size: 12px; font-weight: 600; }
  input[type="number"], input[type="text"], select { box-sizing: border-box; min-height: 36px; padding: 7px 9px; color: var(--text); background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius); font: inherit; }
  input:focus-visible, select:focus-visible, button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .cache-option { display: flex; gap: 8px; max-width: 560px; align-items: flex-start; padding: 9px 0; } .cache-option span { display: grid; gap: 4px; }
  .draft-warning, .selected-plan-note, .resolution-warning, .stale-warning, .proposal-save-warning { margin: 12px 0; padding: 10px 12px; border-left: 3px solid var(--accent); background: rgba(255, 255, 255, .04); }
  .stale-warning { color: var(--danger); border-left-color: var(--danger); }
  button { min-height: 36px; padding: 8px 12px; border-radius: var(--radius); font: inherit; cursor: pointer; } button:disabled { cursor: default; opacity: .55; }
  .primary { color: var(--bg); background: var(--accent); border: 1px solid var(--accent); font-weight: 700; } .secondary { color: var(--text); background: transparent; border: 1px solid var(--border); }
  .text-button { padding: 2px 0; color: var(--accent); background: transparent; border: 0; font-size: 12px; }
  .error { display: grid; gap: 5px; margin-top: 16px; padding: 12px; color: var(--text); background: rgba(255, 107, 107, .1); border: 1px solid var(--danger); border-radius: var(--radius); } .error span { color: #ffb0b0; }
  .notice { margin-top: 12px; color: var(--accent); font-size: 13px; }
  .header-actions, .proposal-actions { display: flex; gap: 8px; flex-wrap: wrap; flex-shrink: 0; }
  .header-actions button, .proposal-actions button { flex: 0 0 auto; overflow-wrap: normal; }
  .status-pill, .step-number, .work-item { color: var(--text-muted); font-size: 11px; }
  .status-pill { padding: 5px 8px; background: var(--bg); border: 1px solid var(--border); border-radius: 999px; white-space: nowrap; }
  .context-list, .timing-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; margin: 0 0 14px; } .context-list div, .timing-grid div { min-width: 0; padding: 9px; background: var(--bg); border-radius: var(--radius); } dt { color: var(--text-muted); font-size: 11px; } dd { margin-top: 3px; color: var(--text); font-size: 12px; overflow-wrap: anywhere; }
  .work-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 7px; margin: 12px 0 16px; } .work-item { padding: 8px; background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius); } .work-item.required { color: var(--accent); border-color: var(--accent); }
  .proposal-list { display: grid; gap: 10px; } .proposal-step { padding: 14px; background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius); } .proposal-step.active-conflict { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }
  .step-heading { display: flex; gap: 10px; align-items: flex-start; } .step-number { display: grid; flex: 0 0 24px; place-items: center; width: 24px; height: 24px; color: var(--bg); background: var(--accent); border-radius: 50%; font-weight: 700; } .operation-summary { margin-top: 4px; color: var(--text-muted); font-size: 12px; }
  .comparison { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; margin: 12px 0; } .comparison > div, .candidate-detail, .mapped-list { padding: 10px; border: 1px solid var(--border); border-radius: var(--radius); } .comparison span, .candidate-detail span, .subheading { color: var(--text-muted); font-size: 11px; font-weight: 700; text-transform: uppercase; } .comparison p, .candidate-detail p, .mapped-list p { margin-top: 5px; font-size: 13px; }
  .resolution { display: grid; gap: 10px; margin-top: 12px; } .resolution-row { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; padding: 10px; border: 1px dashed var(--border); border-radius: var(--radius); } .resolution-row.two-column { grid-template-columns: repeat(2, minmax(0, 1fr)); } .field.wide { min-width: 0; } .candidate-detail { grid-column: 1 / -1; }
  .candidate-search { max-width: 560px; } .candidate-search small { color: var(--text-muted); font-size: 11px; }
  .mapped-list { margin-top: 10px; } .mapped-list p + p { padding-top: 7px; border-top: 1px solid var(--border); } .blocked-note { margin-top: 12px; }
  .accept-row { align-items: center; margin-top: 16px; padding-top: 14px; border-top: 1px solid var(--border); } .accept-row p { margin: 0; }
  .timing-grid { grid-template-columns: repeat(4, minmax(0, 1fr)); }
  .empty { color: var(--text-muted); font-size: 13px; }
  @media (max-width: 700px) { .header, .section-heading, .accept-row { flex-direction: column; } .mode-list, .work-grid, .comparison, .resolution-row, .resolution-row.two-column, .timing-grid, .context-list { grid-template-columns: 1fr; } .header-actions, .proposal-actions { width: 100%; } .header-actions button, .proposal-actions button { flex: 1; } }
</style>
