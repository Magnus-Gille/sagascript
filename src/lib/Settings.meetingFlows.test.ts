// Rendered Settings flows for meeting import controls and saved reviews.
import { emit } from "@tauri-apps/api/event";
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import meetingJobFixtures from "../../contracts/transport/get_meeting_job.json";
import { deferred } from "../test/deferred";
import { backendError, type CommandHandlers } from "../test/ipc";
import { renderSettings, settingsBackend, settingsValue, swedishProfile } from "../test/settings";
import type { MeetingJobSnapshot } from "./api";
import type { MeetingReview, MeetingReviewState } from "./meeting-types";

const jobId = "settings-meeting-flow";
const completedSnapshot = {
  ...(meetingJobFixtures.completed as unknown as MeetingJobSnapshot),
  id: jobId,
};
const runningSnapshot = {
  ...(meetingJobFixtures.running_preparing as unknown as MeetingJobSnapshot),
  id: jobId,
};
const terminalSnapshots = [
  ["completed", completedSnapshot],
  ["failed", { ...(meetingJobFixtures.failed as unknown as MeetingJobSnapshot), id: jobId }],
  ["cancelled", { ...(meetingJobFixtures.cancelled as unknown as MeetingJobSnapshot), id: jobId }],
] as const;

const transcript = completedSnapshot.transcript;
if (!transcript) throw new Error("The completed meeting transport fixture needs a transcript.");
const review: MeetingReview = {
  schema_version: 1,
  original: transcript,
  original_revision: "7201d4801197c586ca6dafaa51ca6d40509b4646142653870f937ec9a7c38253",
  generation: 0,
  batches: [],
  revision: "668400f77910581ef62e7e03f6dc83de085e2e1d8f0a7b4186856be189c50b35",
};
const savedReview: MeetingReviewState = { review, transcript };

const drop = (paths: string[]) => emit("tauri://drag-drop", { paths, position: { x: 0, y: 0 } });
const queueSummary = () => screen.getByRole("status", { name: "File transcription status" });

async function openTranscribeTab(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "Transcribe" }));
}

function meetingSettingsBackend(overrides: CommandHandlers = {}) {
  return settingsBackend({
    get_settings: () => settingsValue({ hotkey_profiles: [swedishProfile] }),
    set_update_result_pending: () => undefined,
    ...overrides,
  });
}

test.each(terminalSnapshots)("meeting options stay disabled through begin and polling until %s", async (_status, terminal) => {
  const beginStarted = deferred<void>();
  const beginAcknowledgement = deferred<string>();
  const firstPollStarted = deferred<void>();
  const terminalPollStarted = deferred<void>();
  const terminalPoll = deferred<MeetingJobSnapshot>();
  let pollCount = 0;
  const { ipc, view } = await renderSettings(
    meetingSettingsBackend({
      begin_meeting_file: () => {
        beginStarted.resolve();
        return beginAcknowledgement.promise;
      },
      get_meeting_job: () => {
        pollCount += 1;
        if (pollCount === 1) {
          firstPollStarted.resolve();
          return runningSnapshot;
        }
        terminalPollStarted.resolve();
        return terminalPoll.promise;
      },
      create_meeting_review: () => savedReview,
    }),
  );
  const user = userEvent.setup();
  const diarization = () => screen.getByRole("checkbox", { name: "Speaker diarization" });
  const context = () => screen.getByRole("textbox", { name: "Extra context for this file" });
  const profile = () => screen.getByRole("combobox", { name: "Profile (optional)" });

  try {
    await openTranscribeTab(user);
    await user.click(diarization());
    await user.type(context(), "Interview speakers");
    await drop(["/recordings/interview.wav"]);
    await beginStarted.promise;

    // The queue owns the busy state while begin_meeting_file is unresolved.
    expect(diarization()).toBeDisabled();
    expect(context()).toBeDisabled();
    expect(profile()).toBeDisabled();

    vi.useFakeTimers();
    beginAcknowledgement.resolve(jobId);
    await firstPollStarted.promise;
    expect(ipc.callsTo("get_meeting_job")).toEqual([{ jobId }]);
    expect(diarization()).toBeDisabled();
    expect(context()).toBeDisabled();
    expect(profile()).toBeDisabled();

    await vi.advanceTimersByTimeAsync(1_000);
    await terminalPollStarted.promise;
    expect(ipc.callsTo("get_meeting_job")).toEqual([{ jobId }, { jobId }]);
    expect(diarization()).toBeDisabled();
    expect(context()).toBeDisabled();
    expect(profile()).toBeDisabled();

    vi.useRealTimers();
    terminalPoll.resolve(terminal);
    await waitFor(() => {
      expect(diarization()).toBeEnabled();
      expect(context()).toBeEnabled();
      expect(profile()).toBeEnabled();
    });
    expect(ipc.callsTo("begin_meeting_file")).toEqual([
      { filePath: "/recordings/interview.wav", prompt: "Interview speakers", profileId: "default" },
    ]);
    expect(ipc.callsTo("transcribe_file")).toHaveLength(0);
  } finally {
    vi.useRealTimers();
    beginAcknowledgement.resolve(jobId);
    terminalPoll.resolve(terminal);
    view.unmount();
  }
});

test("Open saved meeting renders the returned review without starting a transcription", async () => {
  const { ipc } = await renderSettings(meetingSettingsBackend({ open_meeting_review: () => savedReview }));
  const user = userEvent.setup();
  await openTranscribeTab(user);
  await user.click(screen.getByRole("button", { name: "Open saved meeting..." }));

  expect(await screen.findByDisplayValue(transcript.segments[0].text)).toBeInTheDocument();
  expect(ipc.callsTo("open_meeting_review")).toEqual([{}]);
  expect(ipc.callsTo("begin_meeting_file")).toHaveLength(0);
  expect(ipc.callsTo("transcribe_file")).toHaveLength(0);
});

test("cancelling the saved-review picker leaves an empty editor and a cancelled job", async () => {
  const { ipc } = await renderSettings(meetingSettingsBackend({ open_meeting_review: () => null }));
  const user = userEvent.setup();
  await openTranscribeTab(user);
  await user.click(screen.getByRole("button", { name: "Open saved meeting..." }));

  await waitFor(() => expect(queueSummary()).toHaveTextContent(/^0 completed · 0 failed · 1 cancelled/));
  expect(screen.getByRole("button", { name: "Open saved meeting..." })).toBeEnabled();
  expect(ipc.callsTo("open_meeting_review")).toEqual([{}]);
  expect(screen.queryByDisplayValue(transcript.segments[0].text)).toBeNull();
  expect(screen.queryByRole("alert")).toBeNull();
  expect(ipc.callsTo("begin_meeting_file")).toHaveLength(0);
  expect(ipc.callsTo("transcribe_file")).toHaveLength(0);
});

test("Open saved meeting reports an IPC error and does not start a transcription", async () => {
  const { ipc } = await renderSettings(
    meetingSettingsBackend({
      open_meeting_review: () => {
        throw backendError("unavailable", "No saved review is available");
      },
    }),
  );
  const user = userEvent.setup();
  await openTranscribeTab(user);
  await user.click(screen.getByRole("button", { name: "Open saved meeting..." }));

  expect(await screen.findByText("No saved review is available")).toBeInTheDocument();
  expect(ipc.callsTo("open_meeting_review")).toEqual([{}]);
  expect(ipc.callsTo("begin_meeting_file")).toHaveLength(0);
  expect(ipc.callsTo("transcribe_file")).toHaveLength(0);
});
