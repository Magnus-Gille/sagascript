import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import type { ComponentProps } from "svelte";
import MeetingReprocessing from "./MeetingReprocessing.svelte";
import type { CorrectionOperation, MeetingReview, MeetingTranscript } from "./meeting-types";
import type { ProposalState } from "./meeting-reprocessing-types";

// What a proposal's revision resets: the candidate search and the unapplied resolutions,
// the action error, and the mark that the proposal was saved. A new object with the same
// revision resets none of them.

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

const editOperation: Extract<CorrectionOperation, { kind: "edit_segment" }> = {
  kind: "edit_segment",
  segment_id: "old-segment",
  text: "Corrected words",
};

function proposalState(revision: string, steps: ProposalState["preview"]["steps"]): ProposalState {
  return {
    proposal: {
      schema_version: 1,
      previous: review,
      proposed: transcript,
      generation: 2,
      resolutions: steps.map((step) => step.mapped),
      revision,
    },
    preview: { candidate: review, steps },
    candidate: transcript,
  };
}

function conflictProposal(revision = "proposal-1"): ProposalState {
  return proposalState(revision, [{ original: editOperation, mapped: null, status: "conflict" }]);
}

function appliedProposal(revision = "proposal-1"): ProposalState {
  return proposalState(revision, [
    { original: editOperation, mapped: [{ ...editOperation, segment_id: "candidate-1" }], status: "applied" },
  ]);
}

function renderReprocessing(proposal: ProposalState) {
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
    proposal,
    result: null,
    ...callbacks,
  };
  const view = render(MeetingReprocessing, props);
  return {
    ...callbacks,
    showProposal: async (next: ProposalState | null) => {
      props = { ...props, proposal: next };
      await view.rerender(props);
    },
  };
}

function stubConfirm(answer: boolean) {
  const confirm = vi.fn().mockReturnValue(answer);
  vi.stubGlobal("confirm", confirm);
  Object.defineProperty(window, "confirm", { configurable: true, value: confirm });
  return confirm;
}

function candidateSearch(): HTMLElement {
  return screen.getByRole("textbox", { name: /^Find candidate segments/ });
}

function candidateTarget(): HTMLElement {
  return within(screen.getByLabelText("Resolve conflict 1")).getByRole("combobox", { name: "Candidate segment" });
}

async function searchAndChooseTarget(): Promise<void> {
  const user = userEvent.setup();
  await user.type(candidateSearch(), "Budget");
  await user.selectOptions(candidateTarget(), "candidate-2");
  expect(candidateSearch()).toHaveValue("Budget");
  expect(candidateTarget()).toHaveValue("candidate-2");
  expect(screen.getByRole("button", { name: "Apply this resolution" })).toBeEnabled();
}

test("a proposal with a new revision clears the candidate search and the unapplied resolution", async () => {
  const view = renderReprocessing(conflictProposal());
  await searchAndChooseTarget();

  await view.showProposal(conflictProposal("proposal-2"));

  expect(candidateSearch()).toHaveValue("");
  expect(candidateTarget()).toHaveValue("");
  expect(screen.getByRole("button", { name: "Apply this resolution" })).toBeDisabled();
  expect(view.onResolve).not.toHaveBeenCalled();
});

test("a new proposal object with the same revision keeps the candidate search and the unapplied resolution", async () => {
  const view = renderReprocessing(conflictProposal());
  await searchAndChooseTarget();

  await view.showProposal(conflictProposal());

  expect(candidateSearch()).toHaveValue("Budget");
  expect(candidateTarget()).toHaveValue("candidate-2");
  await userEvent.setup().click(screen.getByRole("button", { name: "Apply this resolution" }));
  expect(view.onResolve).toHaveBeenCalledWith(0, [{ kind: "edit_segment", segment_id: "candidate-2", text: "Corrected words" }]);
});

test("a proposal that was removed comes back with an empty candidate search and resolution", async () => {
  const view = renderReprocessing(conflictProposal());
  await searchAndChooseTarget();

  await view.showProposal(null);
  expect(screen.queryByRole("heading", { name: "2. Review migration proposal" })).not.toBeInTheDocument();
  await view.showProposal(conflictProposal());

  expect(candidateSearch()).toHaveValue("");
  expect(candidateTarget()).toHaveValue("");
  expect(screen.getByRole("button", { name: "Apply this resolution" })).toBeDisabled();
});

test("an action error stays for the same revision and is cleared by a new one", async () => {
  const view = renderReprocessing(appliedProposal());
  view.onSave.mockRejectedValueOnce(new Error("The proposal could not be written."));

  await userEvent.setup().click(screen.getByRole("button", { name: "Save proposal" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Reprocessing notice");

  await view.showProposal(appliedProposal());
  expect(screen.getByRole("alert")).toHaveTextContent("Reprocessing notice");

  await view.showProposal(appliedProposal("proposal-2"));
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

test("a saved proposal that returns after another revision must be confirmed again before accepting", async () => {
  const view = renderReprocessing(appliedProposal());
  const confirm = stubConfirm(false);
  const user = userEvent.setup();

  await user.click(screen.getByRole("button", { name: "Save proposal" }));
  expect(await screen.findByText("Reprocessing proposal saved.")).toBeInTheDocument();

  await view.showProposal(appliedProposal());
  await user.click(screen.getByRole("button", { name: "Accept proposal" }));
  expect(confirm).not.toHaveBeenCalled();
  expect(view.onAccept).toHaveBeenCalledOnce();

  view.onAccept.mockClear();
  await view.showProposal(appliedProposal("proposal-2"));
  await view.showProposal(appliedProposal());
  await user.click(screen.getByRole("button", { name: "Accept proposal" }));
  expect(confirm).toHaveBeenCalledWith(
    "Accepting this proposal replaces the current review. Save the proposal first if you may need to recover it. Continue?",
  );
  expect(view.onAccept).not.toHaveBeenCalled();
});
