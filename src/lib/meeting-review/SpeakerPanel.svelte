<script lang="ts">
  import type { MeetingSpeaker, MeetingTranscript } from "../meeting-types";

  interface Props {
    idPrefix: string;
    transcript: MeetingTranscript;
    labelDrafts: Record<string, string>;
    mergeTargets: Record<string, string>;
    disabled: () => boolean;
    renameSpeaker: (speaker: MeetingSpeaker) => Promise<void>;
    mergeSpeaker: (from: string) => Promise<void>;
    discardMerge: (speakerId: string) => void;
    // The drafts belong to the review: edits are written back through these so the parent stays their only writer.
    setLabelDraft: (speakerId: string, label: string) => void;
    setMergeTarget: (speakerId: string, target: string) => void;
  }

  let {
    idPrefix,
    transcript,
    labelDrafts,
    mergeTargets,
    disabled,
    renameSpeaker,
    mergeSpeaker,
    discardMerge,
    setLabelDraft,
    setMergeTarget,
  }: Props = $props();
</script>

<section class="speaker-panel" aria-labelledby={idPrefix + "speaker-panel-title"}>
  <div class="section-heading">
    <div>
      <h2 id={idPrefix + "speaker-panel-title"}>Speakers</h2>
      <p>Rename a speaker or merge two speaker IDs when they belong to the same person.</p>
    </div>
  </div>
  {#if transcript.speakers.length === 0}
    <p class="empty">No speaker labels are available for this transcript.</p>
  {:else}
    <div class="speaker-list">
      {#each transcript.speakers as speaker, index (speaker.id)}
        <div class="speaker-row">
          <div class="speaker-identity">
            <span class="speaker-dot" aria-hidden="true">{index + 1}</span>
            <div>
              <label for={idPrefix + "speaker-name-" + index}>Speaker {index + 1} name</label>
              <input
                id={idPrefix + "speaker-name-" + index}
                type="text"
                value={labelDrafts[speaker.id] ?? speaker.label}
                aria-label={"Rename " + speaker.label}
                disabled={disabled()}
                oninput={(event) => setLabelDraft(speaker.id, (event.currentTarget as HTMLInputElement).value)}
                onkeydown={(event) => {
                  if (event.key === "Enter") void renameSpeaker(speaker);
                }}
              />
            </div>
          </div>
          <div class="speaker-actions">
            <button
              type="button"
              class="secondary"
              disabled={disabled() || !(labelDrafts[speaker.id] ?? "").trim() || (labelDrafts[speaker.id] ?? "").trim() === speaker.label}
              onclick={() => void renameSpeaker(speaker)}
            >
              Rename
            </button>
            {#if transcript.speakers.length > 1}
              <label class="merge-control">
                <span>Merge into</span>
                <select
                  aria-label={"Merge " + speaker.label + " into"}
                  value={mergeTargets[speaker.id] ?? ""}
                  disabled={disabled()}
                  onchange={(event) => setMergeTarget(speaker.id, (event.currentTarget as HTMLSelectElement).value)}
                >
                  <option value="">Choose speaker</option>
                  {#each transcript.speakers as target (target.id)}
                    {#if target.id !== speaker.id}
                      <option value={target.id}>{target.label}</option>
                    {/if}
                  {/each}
                </select>
              </label>
              <button
                type="button"
                class="secondary"
                disabled={disabled() || !mergeTargets[speaker.id]}
                onclick={() => void mergeSpeaker(speaker.id)}
              >
                Merge
              </button>
              {#if mergeTargets[speaker.id]}
                <button
                  type="button"
                  class="secondary"
                  disabled={disabled()}
                  aria-label={"Clear merge selection for " + speaker.label}
                  onclick={() => discardMerge(speaker.id)}
                >
                  Clear selection
                </button>
              {/if}
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}
</section>

<style>
  .section-heading,
  .speaker-row {
    display: flex;
    gap: 12px;
  }
  .section-heading {
    align-items: flex-start;
    justify-content: space-between;
  }
  h2 {
    font-size: 16px;
  }
  .section-heading p,
  .empty,
  label,
  .merge-control span {
    color: var(--text-muted);
    font-size: 12px;
  }
  .speaker-panel {
    margin-top: 24px;
  }
  .section-heading {
    margin-bottom: 12px;
  }
  .section-heading p {
    margin-top: 3px;
  }
  .speaker-list {
    display: grid;
    gap: 8px;
  }
  .speaker-row {
    align-items: center;
    justify-content: space-between;
    padding: 12px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .speaker-identity {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .speaker-identity > div {
    min-width: 0;
  }
  .speaker-dot {
    display: grid;
    flex: 0 0 28px;
    place-items: center;
    width: 28px;
    height: 28px;
    color: var(--bg);
    background: var(--accent);
    border-radius: 50%;
    font-size: 12px;
    font-weight: 700;
  }
  .speaker-identity input {
    width: min(220px, 100%);
    margin-top: 3px;
  }
  .speaker-actions {
    display: flex;
    align-items: end;
    justify-content: flex-end;
    gap: 8px;
    flex-wrap: wrap;
  }
  .speaker-actions button {
    white-space: nowrap;
  }
  .merge-control {
    display: grid;
    gap: 3px;
    min-width: 140px;
  }
  .merge-control select {
    min-width: 0;
  }
  @media (max-width: 600px) {
    .section-heading,
    .speaker-row {
      flex-direction: column;
    }
    .speaker-actions {
      display: grid;
      grid-template-columns: minmax(0, 1fr);
      align-items: stretch;
      width: 100%;
    }
    .speaker-actions > button,
    .merge-control {
      width: 100%;
      min-width: 0;
    }
  }
</style>
