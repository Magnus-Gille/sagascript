<script lang="ts">
  import type { CorrectionOperation, MeetingSegment, MeetingSpeaker } from "../meeting-types";
  import type { ProposalState } from "../meeting-reprocessing-types";
  import { filterCandidateSegments } from "../meeting-reprocessing-state";

  interface Props {
    idPrefix: string;
    proposal: ProposalState;
    pendingAction: string | null;
    draftDirty: boolean;
    isBusy: () => boolean;
    runAction: (name: string, action: () => Promise<boolean | void>) => Promise<boolean>;
    onResolve: (index: number, operations: CorrectionOperation[]) => Promise<void>;
    discard: () => void;
    save: () => Promise<void>;
    accept: () => Promise<void>;
    proposalIsFresh: () => boolean;
    proposalWasSaved: () => boolean;
    allApplied: () => boolean;
    canAccept: () => boolean;
  }

  type ResolutionOperation = CorrectionOperation;

  let {
    idPrefix,
    proposal,
    pendingAction,
    draftDirty,
    isBusy,
    runAction,
    onResolve,
    discard,
    save,
    accept,
    proposalIsFresh,
    proposalWasSaved,
    allApplied,
    canAccept,
  }: Props = $props();

  let proposalRevision = $state<string | null>(null);
  let candidateSegmentQuery = $state("");
  let resolutionDrafts = $state<Record<number, ResolutionOperation[]>>({});

  $effect(() => {
    const revision = proposal?.proposal.revision ?? null;
    if (revision === proposalRevision) return;
    proposalRevision = revision;
    candidateSegmentQuery = "";
    resolutionDrafts = {};
  });

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
      if (operation.text != null) changes.push(`text → “${operation.text}”`);
      if (operation.speaker_id != null) changes.push(`speaker → ${operation.speaker_id}`);
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
        ...(operation.text != null ? { text: operation.text } : {}),
        ...(operation.speaker_id != null ? { speaker_id: operation.speaker_id } : {}),
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
      const targetIds = rows.map((row) => (row.kind === "edit_segment" ? row.segment_id : ""));
      return (
        rows.every(
          (row) =>
            row.kind === "edit_segment" &&
            segmentIds.has(row.segment_id) &&
            (row.text != null || row.speaker_id != null) &&
            (row.speaker_id == null || speakerIds.has(row.speaker_id)),
        ) && new Set(targetIds).size === targetIds.length
      );
    }
    if (original.kind === "rename_speaker") {
      const row = rows[0];
      return row.kind === "rename_speaker" && speakerIds.has(row.speaker_id) && row.label.trim().length > 0;
    }
    const row = rows[0];
    return row.kind === "merge_speakers" && speakerIds.has(row.from_id) && speakerIds.has(row.into_id) && row.from_id !== row.into_id;
  }

  async function resolve(index: number, original: CorrectionOperation): Promise<void> {
    if (draftDirty || !validResolution(index, original)) return;
    await runAction(`resolve-${index}`, () => onResolve(index, resolutionRows(index, original)));
  }

  function firstConflictIndex(): number {
    return proposal?.preview.steps.findIndex((step) => step.status === "conflict") ?? -1;
  }
</script>

<section class="panel proposal-panel" aria-labelledby={idPrefix + "proposal-title"}>
  <div class="section-heading">
    <div>
      <h2 id={idPrefix + "proposal-title"}>2. Review migration proposal</h2>
      <p>
        {proposal.preview.steps.length} correction step{proposal.preview.steps.length === 1 ? "" : "s"}. Automatic mappings stay read-only;
        conflicts require an explicit target.
      </p>
    </div>
    <div class="proposal-actions">
      <button type="button" class="secondary" disabled={isBusy()} onclick={discard}>Discard proposal</button>
      <button type="button" class="secondary" disabled={isBusy()} onclick={() => void save()}
        >{pendingAction === "save" ? "Saving…" : "Save proposal"}</button
      >
    </div>
  </div>
  {#if !proposalIsFresh()}
    <p class="stale-warning" role="alert">
      This proposal was made from an older review revision. Save/export remains available, but accepting is disabled; generate a new
      proposal for the current review.
    </p>
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
              <strong
                >{step.status === "applied"
                  ? "Applied automatically"
                  : step.status === "conflict"
                    ? "Conflict needs a decision"
                    : "Blocked until the earlier conflict is resolved"}</strong
              >
              <p class="operation-summary">Original: {operationSummary(step.original)}</p>
            </div>
          </div>

          {#if source}
            <div class="comparison" aria-label="Original and candidate segment comparison">
              <div>
                <span>Previous original · {formatSeconds(source.start)}–{formatSeconds(source.end)}</span>
                <p>{source.text || "(empty text)"}</p>
              </div>
              <div>
                <span>Candidate transcript</span>
                <p>Select an explicit target below; candidate text and timing will appear here.</p>
              </div>
            </div>
          {/if}

          {#if step.status === "applied"}
            <div class="mapped-list">
              <span class="subheading">Mapped disposition</span>
              {#each step.mapped ?? [] as mapped, mappedIndex (mappedIndex)}<p>{operationSummary(mapped)}</p>{/each}
            </div>
          {:else if activeConflict}
            <div class="resolution" aria-label={`Resolve conflict ${index + 1}`}>
              <span class="subheading">Choose candidate target{step.original.kind === "edit_segment" ? "s" : ""}</span>
              {#if step.original.kind === "edit_segment"}
                <label class="field wide candidate-search">
                  <span>Find candidate segments</span>
                  <input
                    type="text"
                    value={candidateSegmentQuery}
                    placeholder="Search text, id, or time (for example 12:34)"
                    disabled={isBusy() || draftDirty}
                    oninput={(event) => {
                      candidateSegmentQuery = (event.currentTarget as HTMLInputElement).value;
                    }}
                  />
                  <small
                    >{filterCandidateSegments(candidateSegments(), candidateSegmentQuery).length} matching candidate{filterCandidateSegments(
                      candidateSegments(),
                      candidateSegmentQuery,
                    ).length === 1
                      ? ""
                      : "s"}; selected targets remain visible.</small
                  >
                </label>
              {/if}
              {#each resolutionRows(index, step.original) as operation, rowIndex (rowIndex)}
                {#if operation.kind === "edit_segment"}
                  <div class="resolution-row">
                    <label class="field wide">
                      <span>Candidate segment</span>
                      <select
                        value={operation.segment_id}
                        onchange={(event) =>
                          updateResolution(index, rowIndex, {
                            ...operation,
                            segment_id: (event.currentTarget as HTMLSelectElement).value,
                          })}
                      >
                        <option value="">Choose a candidate segment</option>
                        {#each filteredCandidateSegments(operation.segment_id) as segment (segment.id)}
                          <option value={segment.id}
                            >{formatSeconds(segment.start)}–{formatSeconds(segment.end)} · {segment.text || "(empty text)"}</option
                          >
                        {/each}
                      </select>
                    </label>
                    <label class="field wide">
                      <span>Replacement text {operation.text == null ? "(unchanged)" : ""}</span>
                      <input
                        type="text"
                        value={operation.text ?? ""}
                        oninput={(event) =>
                          updateResolution(index, rowIndex, { ...operation, text: (event.currentTarget as HTMLInputElement).value })}
                      />
                    </label>
                    <label class="field wide">
                      <span>Speaker {operation.speaker_id == null ? "(unchanged)" : ""}</span>
                      <select
                        value={operation.speaker_id ?? ""}
                        onchange={(event) => {
                          const speakerId = (event.currentTarget as HTMLSelectElement).value;
                          updateResolution(
                            index,
                            rowIndex,
                            speakerId
                              ? { ...operation, speaker_id: speakerId }
                              : {
                                  kind: "edit_segment",
                                  segment_id: operation.segment_id,
                                  ...(operation.text != null ? { text: operation.text } : {}),
                                },
                          );
                        }}
                      >
                        <option value="">Keep candidate speaker</option>
                        {#each candidateSpeakers() as speaker (speaker.id)}<option value={speaker.id}>{speaker.label} ({speaker.id})</option
                          >{/each}
                      </select>
                    </label>
                    {#if candidateSegment(operation.segment_id)}
                      {@const target = candidateSegment(operation.segment_id)}
                      <div class="candidate-detail">
                        <span>Candidate · {formatSeconds(target!.start)}–{formatSeconds(target!.end)}</span>
                        <p>{target!.text || "(empty text)"}</p>
                      </div>
                    {/if}
                    {#if resolutionRows(index, step.original).length > 1}
                      <button type="button" class="text-button" onclick={() => removeTarget(index, rowIndex, step.original)}
                        >Remove target</button
                      >
                    {/if}
                  </div>
                {:else if operation.kind === "rename_speaker"}
                  <div class="resolution-row two-column">
                    <label class="field">
                      <span>Candidate speaker</span>
                      <select
                        value={operation.speaker_id}
                        onchange={(event) =>
                          updateResolution(index, rowIndex, {
                            ...operation,
                            speaker_id: (event.currentTarget as HTMLSelectElement).value,
                          })}
                      >
                        <option value="">Choose a candidate speaker</option>
                        {#each candidateSpeakers() as speaker (speaker.id)}<option value={speaker.id}>{speaker.label} ({speaker.id})</option
                          >{/each}
                      </select>
                    </label>
                    <label class="field"
                      ><span>New label</span><input
                        type="text"
                        value={operation.label}
                        oninput={(event) =>
                          updateResolution(index, rowIndex, { ...operation, label: (event.currentTarget as HTMLInputElement).value })}
                      /></label
                    >
                  </div>
                {:else}
                  <div class="resolution-row two-column">
                    <label class="field">
                      <span>Speaker to merge</span>
                      <select
                        value={operation.from_id}
                        onchange={(event) =>
                          updateResolution(index, rowIndex, {
                            ...operation,
                            from_id: (event.currentTarget as HTMLSelectElement).value,
                          })}
                      >
                        <option value="">Choose source speaker</option>
                        {#each candidateSpeakers() as speaker (speaker.id)}<option value={speaker.id}>{speaker.label} ({speaker.id})</option
                          >{/each}
                      </select>
                    </label>
                    <label class="field">
                      <span>Merge into</span>
                      <select
                        value={operation.into_id}
                        onchange={(event) =>
                          updateResolution(index, rowIndex, {
                            ...operation,
                            into_id: (event.currentTarget as HTMLSelectElement).value,
                          })}
                      >
                        <option value="">Choose target speaker</option>
                        {#each candidateSpeakers() as speaker (speaker.id)}<option value={speaker.id}>{speaker.label} ({speaker.id})</option
                          >{/each}
                      </select>
                    </label>
                  </div>
                {/if}
              {/each}
              {#if step.original.kind === "edit_segment"}
                <button type="button" class="secondary" disabled={isBusy()} onclick={() => addTarget(index, step.original)}
                  >Add another candidate target</button
                >
              {/if}
              {#if !validResolution(index, step.original)}<p class="resolution-warning">
                  Choose at least one valid candidate target. Empty resolutions are not accepted.
                </p>{/if}
              <button
                type="button"
                class="primary"
                disabled={isBusy() || draftDirty || !validResolution(index, step.original)}
                onclick={() => void resolve(index, step.original)}
              >
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

<style>
  .section-heading,
  .accept-row {
    display: flex;
    gap: 16px;
    align-items: flex-start;
    justify-content: space-between;
  }
  h2 {
    font-size: 16px;
  }
  .section-heading p,
  .field small,
  .blocked-note,
  .resolution-warning,
  .accept-row p {
    color: var(--text-muted);
    font-size: 12px;
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
  .field {
    display: grid;
    gap: 5px;
    min-width: 160px;
  }
  .field > span {
    color: var(--text);
    font-size: 12px;
    font-weight: 600;
  }
  input[type="text"],
  select {
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
  select:focus-visible,
  button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .resolution-warning,
  .stale-warning,
  .proposal-save-warning {
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
  .text-button {
    padding: 2px 0;
    color: var(--accent);
    background: transparent;
    border: 0;
    font-size: 12px;
  }
  .proposal-actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    flex-shrink: 0;
  }
  .proposal-actions button {
    flex: 0 0 auto;
    overflow-wrap: normal;
  }
  .step-number {
    color: var(--text-muted);
    font-size: 11px;
  }
  .proposal-list {
    display: grid;
    gap: 10px;
  }
  .proposal-step {
    padding: 14px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .proposal-step.active-conflict {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .step-heading {
    display: flex;
    gap: 10px;
    align-items: flex-start;
  }
  .step-number {
    display: grid;
    flex: 0 0 24px;
    place-items: center;
    width: 24px;
    height: 24px;
    color: var(--bg);
    background: var(--accent);
    border-radius: 50%;
    font-weight: 700;
  }
  .operation-summary {
    margin-top: 4px;
    color: var(--text-muted);
    font-size: 12px;
  }
  .comparison {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
    margin: 12px 0;
  }
  .comparison > div,
  .candidate-detail,
  .mapped-list {
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .comparison span,
  .candidate-detail span,
  .subheading {
    color: var(--text-muted);
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
  }
  .comparison p,
  .candidate-detail p,
  .mapped-list p {
    margin-top: 5px;
    font-size: 13px;
  }
  .resolution {
    display: grid;
    gap: 10px;
    margin-top: 12px;
  }
  .resolution-row {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 8px;
    padding: 10px;
    border: 1px dashed var(--border);
    border-radius: var(--radius);
  }
  .resolution-row.two-column {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .field.wide {
    min-width: 0;
  }
  .candidate-detail {
    grid-column: 1 / -1;
  }
  .candidate-search {
    max-width: 560px;
  }
  .candidate-search small {
    color: var(--text-muted);
    font-size: 11px;
  }
  .mapped-list {
    margin-top: 10px;
  }
  .mapped-list p + p {
    padding-top: 7px;
    border-top: 1px solid var(--border);
  }
  .blocked-note {
    margin-top: 12px;
  }
  .accept-row {
    align-items: center;
    margin-top: 16px;
    padding-top: 14px;
    border-top: 1px solid var(--border);
  }
  .accept-row p {
    margin: 0;
  }
  .empty {
    color: var(--text-muted);
    font-size: 13px;
  }
  @media (max-width: 700px) {
    .section-heading,
    .accept-row {
      flex-direction: column;
    }
    .comparison,
    .resolution-row,
    .resolution-row.two-column {
      grid-template-columns: 1fr;
    }
    .proposal-actions {
      width: 100%;
    }
    .proposal-actions button {
      flex: 1;
    }
  }
</style>
