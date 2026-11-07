import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import type { ComponentProps } from "svelte";
import MeetingReprocessing from "./MeetingReprocessing.svelte";
import type { CorrectionOperation, MeetingReview, MeetingTranscript } from "./meeting-types";
import type { ProposalState, SelectedReprocessingPlan } from "./meeting-reprocessing-types";

afterEach(() => {
  Reflect.deleteProperty(window, "confirm");
  vi.unstubAllGlobals();
});

const transcript: MeetingTranscript = {
  schema_version: 1,
  source_sha256: "source-sha",
  language: "sv",
  model: "test-model",
  duration_seconds: 12,
  segments: [
    { id: "candidate-1", start: 0, end: 4, text: "Opening remarks", speaker: "speaker-1" },
    { id: "candidate-2", start: 5, end: 9, text: "Budget discussion", speaker: "speaker-2" },
  ],
  speakers: [
    { id: "speaker-1", label: "Alice" },
    { id: "speaker-2", label: "Bo" },
  ],
};

const review: MeetingReview = {
  schema_version: 1,
  original: transcript,
  original_revision: "original-1",
  generation: 1,
  batches: [],
  revision: "review-1",
};

const requiredWork = {
  decode_audio: false,
  transcription: false,
  language_detection: false,
  segmentation: false,
  embeddings: false,
  clustering: true,
};

function selectedPlan(previousRevision = "review-1"): SelectedReprocessingPlan {
  return {
    plan: {
      schema_version: 1,
      mode: "recluster",
      context: {
        source_sha256: "source-sha",
        previous_revision: previousRevision,
        transcription_context_sha256: "transcription-context",
        analysis_context_sha256: "analysis-context",
        cache_sha256: null,
      },
      threshold: 0.5,
      required_work: requiredWork,
      revision: "plan-1",
    },
    file_path: "meeting.wav",
    cache_path: null,
    cache_output: null,
  };
}

function proposalState(steps: ProposalState["preview"]["steps"], previousRevision = "review-1"): ProposalState {
  return {
    proposal: {
      schema_version: 1,
      previous: { ...review, revision: previousRevision },
      proposed: transcript,
      generation: 2,
      resolutions: steps.map((step) => step.mapped),
      revision: "proposal-1",
    },
    preview: {
      candidate: review,
      steps,
    },
    candidate: transcript,
  };
}

const editOperation: Extract<CorrectionOperation, { kind: "edit_segment" }> = {
  kind: "edit_segment",
  segment_id: "old-segment",
  text: "Corrected words",
};

function conflictProposal(stepCount = 1, previousRevision = "review-1"): ProposalState {
  const steps: ProposalState["preview"]["steps"] = Array.from({ length: stepCount }, () => ({
    original: editOperation,
    mapped: null,
    status: "conflict",
  }));
  return proposalState(steps, previousRevision);
}

function appliedProposal(): ProposalState {
  return proposalState([{ original: editOperation, mapped: [{ ...editOperation, segment_id: "candidate-1" }], status: "applied" }]);
}

function appliedProposalWithPrevious(previousRevision: string): ProposalState {
  return proposalState(
    [{ original: editOperation, mapped: [{ ...editOperation, segment_id: "candidate-1" }], status: "applied" }],
    previousRevision,
  );
}

function renderReprocessing(overrides: Partial<ComponentProps<typeof MeetingReprocessing>> = {}) {
  const callbacks = {
    onPlan: vi.fn(async (): Promise<void> => undefined),
    onExecute: vi.fn(async (): Promise<void> => undefined),
    onResolve: vi.fn(async (): Promise<void> => undefined),
    onAccept: vi.fn(async (): Promise<void> => undefined),
    onSave: vi.fn(async (): Promise<boolean> => true),
    onOpen: vi.fn(async (): Promise<void> => undefined),
    onDiscard: vi.fn(),
  };
  let props: ComponentProps<typeof MeetingReprocessing> = {
    busy: false,
    draftDirty: false,
    currentReviewRevision: "review-1",
    selected: null,
    proposal: null,
    result: null,
    ...callbacks,
    ...overrides,
  };
  const view = render(MeetingReprocessing, props);
  return {
    ...callbacks,
    updateProps: async (updates: Partial<ComponentProps<typeof MeetingReprocessing>>) => {
      props = { ...props, ...updates };
      await view.rerender(props);
    },
    unmount: view.unmount,
  };
}

test("execution shows and uses the selected plan even when the next-plan controls differ", async () => {
  const callbacks = renderReprocessing({ selected: selectedPlan() });
  const user = userEvent.setup();

  expect(screen.getByRole("region", { name: "Selected plan" })).toHaveTextContent(
    "This is the plan that Execute will run. Mode: Recluster; threshold: 0.50.",
  );
  await user.click(screen.getByRole("radio", { name: /Full recomputation/ }));
  expect(screen.getByRole("status")).toHaveTextContent("Execution uses the selected plan below (Recluster, threshold 0.50)");
  expect(screen.getByRole("radiogroup", { name: "Reprocessing mode" })).toHaveTextContent("Full recomputation");
  expect(screen.getByRole("region", { name: "Selected plan" })).toHaveTextContent("Mode: Recluster; threshold: 0.50.");

  await user.click(screen.getByRole("button", { name: "Execute selected Recluster plan" }));
  expect(callbacks.onExecute).toHaveBeenCalledOnce();
  expect(callbacks.onPlan).not.toHaveBeenCalled();
});

test("stale selected plans cannot execute against a newer review", async () => {
  const callbacks = renderReprocessing({ selected: selectedPlan("review-old"), currentReviewRevision: "review-new" });
  const execute = screen.getByRole("button", { name: "Execute selected Recluster plan" });

  expect(screen.getByRole("alert")).toHaveTextContent("This plan was made for an older review revision");
  expect(execute).toBeDisabled();
  await userEvent.setup().click(execute);
  expect(callbacks.onExecute).not.toHaveBeenCalled();
});

test("a proposal from an older review cannot be accepted", () => {
  const callbacks = renderReprocessing({ proposal: appliedProposalWithPrevious("review-old") });
  expect(screen.getByRole("alert")).toHaveTextContent("This proposal was made from an older review revision");
  expect(screen.getByRole("alert")).toHaveTextContent("Save/export remains available, but accepting is disabled");
  expect(screen.getByRole("button", { name: "Accept proposal" })).toBeDisabled();
  callbacks.unmount();
});

test("a current proposal with unresolved conflicts cannot be accepted", async () => {
  const callbacks = renderReprocessing({ proposal: conflictProposal() });
  const accept = screen.getByRole("button", { name: "Accept proposal" });
  expect(accept).toBeDisabled();
  expect(screen.getByText("All conflicts must be resolved before accepting this proposal.")).toBeInTheDocument();

  await userEvent.setup().click(accept);
  expect(callbacks.onAccept).not.toHaveBeenCalled();

  const confirm = vi.fn().mockReturnValue(true);
  vi.stubGlobal("confirm", confirm);
  Object.defineProperty(window, "confirm", { configurable: true, value: confirm });
  await callbacks.updateProps({ proposal: appliedProposal() });
  expect(screen.getByRole("button", { name: "Accept proposal" })).toBeEnabled();
  await userEvent.setup().click(screen.getByRole("button", { name: "Accept proposal" }));
  expect(callbacks.onAccept).toHaveBeenCalledOnce();
  callbacks.unmount();
});

test("only the first conflict can be resolved, empty corrections are refused, and target edits are retained", async () => {
  const callbacks = renderReprocessing({ proposal: conflictProposal(2) });
  const user = userEvent.setup();
  const firstResolution = screen.getByLabelText("Resolve conflict 1");
  const applyResolution = within(firstResolution).getByRole("button", { name: "Apply this resolution" });
  expect(
    screen.getByText("Resolve the first conflict before this step can be changed. No correction is silently dropped."),
  ).toBeInTheDocument();
  expect(screen.getAllByRole("button", { name: "Apply this resolution" })).toHaveLength(1);
  expect(
    within(firstResolution).getByText("Choose at least one valid candidate target. Empty resolutions are not accepted."),
  ).toBeInTheDocument();
  expect(applyResolution).toBeDisabled();

  await user.click(within(firstResolution).getByRole("button", { name: "Add another candidate target" }));
  const targetSelectors = within(firstResolution).getAllByRole("combobox", { name: "Candidate segment" });
  await user.selectOptions(targetSelectors[0], "candidate-1");
  const replacementText = within(firstResolution).getAllByRole("textbox", { name: /Replacement text/ })[0];
  await user.clear(replacementText);
  expect(applyResolution).toBeDisabled();
  await user.type(replacementText, "Corrected words");
  await user.selectOptions(targetSelectors[1], "candidate-2");
  expect(
    within(firstResolution)
      .getAllByRole("textbox", { name: /Replacement text/ })
      .map((input) => (input as HTMLInputElement).value),
  ).toEqual(["Corrected words", "Corrected words"]);
  expect(applyResolution).toBeEnabled();
  await user.click(applyResolution);
  expect(callbacks.onResolve).toHaveBeenCalledOnce();
  expect(callbacks.onResolve).toHaveBeenCalledWith(0, [
    { kind: "edit_segment", segment_id: "candidate-1", text: "Corrected words" },
    { kind: "edit_segment", segment_id: "candidate-2", text: "Corrected words" },
  ]);

  callbacks.unmount();
});

test("proposal replacement and discard require confirmation, and cancel keeps the current proposal", async () => {
  const callbacks = renderReprocessing({ proposal: conflictProposal() });
  const confirm = vi.fn();
  vi.stubGlobal("confirm", confirm);
  Object.defineProperty(window, "confirm", { configurable: true, value: confirm });
  const user = userEvent.setup();

  confirm.mockReturnValue(false);
  await user.click(screen.getByRole("button", { name: "Open saved proposal" }));
  expect(confirm).toHaveBeenCalledWith("Open a saved proposal and discard the current proposal resolutions?");
  expect(callbacks.onOpen).not.toHaveBeenCalled();
  expect(screen.getByRole("heading", { name: "2. Review migration proposal" })).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Discard proposal" }));
  expect(confirm).toHaveBeenCalledWith("Discard this reprocessing proposal and all of its resolutions?");
  expect(callbacks.onDiscard).not.toHaveBeenCalled();

  confirm.mockReturnValue(true);
  await user.click(screen.getByRole("button", { name: "Open saved proposal" }));
  expect(callbacks.onOpen).toHaveBeenCalledOnce();
  await user.click(screen.getByRole("button", { name: "Discard proposal" }));
  expect(callbacks.onDiscard).toHaveBeenCalledOnce();
});

test("accepting an unsaved proposal warns before replacing review and saved proposals do not warn", async () => {
  const callbacks = renderReprocessing({ proposal: appliedProposal() });
  expect(screen.getByText("Mapped disposition")).toBeInTheDocument();
  expect(screen.getByText("Edit segment candidate-1: text → “Corrected words”")).toBeInTheDocument();
  const confirm = vi.fn();
  vi.stubGlobal("confirm", confirm);
  Object.defineProperty(window, "confirm", { configurable: true, value: confirm });
  const user = userEvent.setup();

  confirm.mockReturnValue(false);
  await user.click(screen.getByRole("button", { name: "Accept proposal" }));
  expect(confirm).toHaveBeenCalledWith(
    "Accepting this proposal replaces the current review. Save the proposal first if you may need to recover it. Continue?",
  );
  expect(callbacks.onAccept).not.toHaveBeenCalled();

  confirm.mockReturnValue(true);
  await user.click(screen.getByRole("button", { name: "Accept proposal" }));
  expect(callbacks.onAccept).toHaveBeenCalledOnce();

  callbacks.onAccept.mockClear();
  callbacks.onSave.mockResolvedValueOnce(true);
  await user.click(screen.getByRole("button", { name: "Save proposal" }));
  expect(await screen.findByText("Reprocessing proposal saved.")).toBeInTheDocument();
  confirm.mockClear();
  await user.click(screen.getByRole("button", { name: "Accept proposal" }));
  expect(confirm).not.toHaveBeenCalled();
  expect(callbacks.onAccept).toHaveBeenCalledOnce();
  callbacks.unmount();
});

test("a cancelled save does not report success or make the proposal recoverable", async () => {
  const callbacks = renderReprocessing({ proposal: appliedProposal() });
  callbacks.onSave.mockResolvedValue(false);
  const confirm = vi.fn().mockReturnValue(false);
  vi.stubGlobal("confirm", confirm);
  Object.defineProperty(window, "confirm", { configurable: true, value: confirm });

  await userEvent.setup().click(screen.getByRole("button", { name: "Save proposal" }));
  await waitFor(() => expect(callbacks.onSave).toHaveBeenCalledOnce());
  expect(screen.queryByText("Reprocessing proposal saved.")).not.toBeInTheDocument();
  expect(screen.getByText("Save/export this proposal before accepting if you may need to recover the migration.")).toBeInTheDocument();
  await userEvent.setup().click(screen.getByRole("button", { name: "Accept proposal" }));
  expect(confirm).toHaveBeenCalledWith(
    "Accepting this proposal replaces the current review. Save the proposal first if you may need to recover it. Continue?",
  );
  expect(callbacks.onAccept).not.toHaveBeenCalled();
  callbacks.unmount();
});

test("dirty drafts and busy or in-flight actions prevent mutation callbacks and duplicate work", async () => {
  const dirtyPlanCallbacks = renderReprocessing({ selected: selectedPlan() });
  const draftPlanButton = screen.getByRole("button", { name: "Plan reprocessing" });
  const executeButton = screen.getByRole("button", { name: "Execute selected Recluster plan" });
  expect(draftPlanButton).toBeEnabled();
  expect(executeButton).toBeEnabled();
  await dirtyPlanCallbacks.updateProps({ draftDirty: true });
  expect(screen.getByRole("button", { name: "Plan reprocessing" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Execute selected Recluster plan" })).toBeDisabled();
  await userEvent.setup().click(screen.getByRole("button", { name: "Plan reprocessing" }));
  await userEvent.setup().click(screen.getByRole("button", { name: "Execute selected Recluster plan" }));
  expect(dirtyPlanCallbacks.onPlan).not.toHaveBeenCalled();
  expect(dirtyPlanCallbacks.onExecute).not.toHaveBeenCalled();
  dirtyPlanCallbacks.unmount();

  const dirtyAcceptCallbacks = renderReprocessing({ proposal: appliedProposal() });
  const acceptButton = screen.getByRole("button", { name: "Accept proposal" });
  expect(acceptButton).toBeEnabled();
  await dirtyAcceptCallbacks.updateProps({ draftDirty: true });
  expect(screen.getByRole("button", { name: "Accept proposal" })).toBeDisabled();
  await userEvent.setup().click(screen.getByRole("button", { name: "Accept proposal" }));
  expect(dirtyAcceptCallbacks.onAccept).not.toHaveBeenCalled();
  dirtyAcceptCallbacks.unmount();

  const dirtyResolveCallbacks = renderReprocessing({ proposal: conflictProposal() });
  const resolution = screen.getByLabelText("Resolve conflict 1");
  await userEvent.setup().selectOptions(within(resolution).getByRole("combobox", { name: "Candidate segment" }), "candidate-1");
  const resolveButton = within(resolution).getByRole("button", { name: "Apply this resolution" });
  expect(resolveButton).toBeEnabled();
  await dirtyResolveCallbacks.updateProps({ draftDirty: true });
  expect(within(screen.getByLabelText("Resolve conflict 1")).getByRole("button", { name: "Apply this resolution" })).toBeDisabled();
  await userEvent.setup().click(within(screen.getByLabelText("Resolve conflict 1")).getByRole("button", { name: "Apply this resolution" }));
  expect(dirtyResolveCallbacks.onResolve).not.toHaveBeenCalled();
  dirtyResolveCallbacks.unmount();

  const busyCallbacks = renderReprocessing({ busy: true, proposal: conflictProposal() });
  expect(screen.getAllByRole("button", { name: "Open saved proposal" }).at(-1)).toBeDisabled();
  expect(screen.getByRole("button", { name: "Save proposal" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Apply this resolution" })).toBeDisabled();
  await userEvent.setup().click(screen.getByRole("button", { name: "Save proposal" }));
  expect(busyCallbacks.onSave).not.toHaveBeenCalled();
  busyCallbacks.unmount();

  const pendingCallbacks = renderReprocessing({});
  let finishPlan!: () => void;
  pendingCallbacks.onPlan.mockImplementation(
    () =>
      new Promise<void>((resolve) => {
        finishPlan = resolve;
      }),
  );
  const pendingUser = userEvent.setup();
  const planButton = screen.getByRole("button", { name: "Plan reprocessing" });
  await pendingUser.click(planButton);
  expect(screen.getByRole("button", { name: "Planning…" })).toBeDisabled();
  await pendingUser.click(screen.getByRole("button", { name: "Planning…" }));
  expect(pendingCallbacks.onPlan).toHaveBeenCalledOnce();
  finishPlan();
  await waitFor(() => expect(screen.getByRole("button", { name: "Plan reprocessing" })).toBeEnabled());
});
