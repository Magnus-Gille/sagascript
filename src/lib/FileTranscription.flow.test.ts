// Rendered file-import flows through Settings and FileTranscription. These
// tests use the same user controls and typed IPC/events as the webview.
import { emit } from "@tauri-apps/api/event";
import { tick } from "svelte";
import { screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import commandErrors from "../../contracts/transport/command_errors.json";
import type { HotkeyProfile } from "./api";
import { backendError, type IpcMock } from "../test/ipc";
import { deferred, settle } from "../test/deferred";
import { renderSettings, settingsBackend, settingsValue, swedishProfile } from "../test/settings";

const englishProfile: HotkeyProfile = {
  id: "english",
  name: "English",
  shortcut: "Ctrl+Alt+E",
  language: "en",
  push_to_talk_shortcut: null,
  toggle_shortcut: "Ctrl+Alt+E",
};
const multiProfileSettings = () => settingsValue({ hotkey_profiles: [swedishProfile, englishProfile] });
const drop = (paths: string[]) => emit("tauri://drag-drop", { paths, position: { x: 0, y: 0 } });
const queueSummary = () => screen.getByRole("status", { name: "File transcription status" });
const openTranscribe = async (user: ReturnType<typeof userEvent.setup>) => user.click(screen.getByRole("button", { name: "Transcribe" }));

async function configurePlainFile(user: ReturnType<typeof userEvent.setup>, prompt = "Astrid") {
  await user.type(screen.getByRole("textbox", { name: "Extra context for this file" }), prompt);
}

test("dropped files use one run-scoped progress listener and the queue starts only one worker at a time", async () => {
  const first = deferred<string>();
  const second = deferred<string>();
  const duplicate = deferred<string>();
  let oneRuns = 0;
  const ipcHolder: { current?: IpcMock } = {};
  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: ({ filePath }) => {
        // A missed subscription would lose the first native event.
        expect(ipcHolder.current?.listenerCount("plain-transcription-progress")).toBe(1);
        if (filePath === "/audio/one.wav") return ++oneRuns === 1 ? first.promise : duplicate.promise;
        return second.promise;
      },
      set_update_result_pending: () => undefined,
    }),
  );
  ipcHolder.current = ipc;
  const user = userEvent.setup();
  await openTranscribe(user);
  await configurePlainFile(user);
  await drop(["/audio/one.wav", "/audio/two.wav"]);

  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(1));
  const firstCall = ipc.callsTo("transcribe_file")[0];
  expect(firstCall).toMatchObject({
    filePath: "/audio/one.wav",
    prompt: "Astrid",
    profileId: "default",
    diarize: false,
    autoPaste: false,
  });
  expect(firstCall.runId).toEqual(expect.any(String));
  expect(ipc.listenerCount("plain-transcription-progress")).toBe(1);
  expect(queueSummary()).toHaveTextContent(/1 queued · 1 running/);

  // A second real drop of the same path creates a separate queued job, but
  // cannot start another native worker while the first run owns the slot.
  await drop(["/audio/one.wav"]);
  await waitFor(() => expect(queueSummary()).toHaveTextContent(/2 queued · 1 running/));
  expect(ipc.callsTo("transcribe_file")).toHaveLength(1);

  // The old Settings source guard checked that it did not install broad
  // progress listeners. Assert the rendered app's active subscriptions.
  expect(
    ipc.listeners().filter((event) => ["transcription-progress", "transcription-phase", "audio-decode-progress"].includes(event)),
  ).toEqual([]);

  ipc.emit("plain-transcription-progress", { runId: "stale-run", phase: "resampling", percent: 88 });
  await tick();
  expect(screen.getByRole("progressbar", { name: "Read audio" })).toHaveAttribute("aria-valuetext", "Decoding the file");
  ipc.emit("plain-transcription-progress", { runId: firstCall.runId!, phase: "resampling", percent: 40 });
  await waitFor(() => expect(screen.getByRole("progressbar", { name: "Prepare audio & model" })).toHaveAttribute("aria-valuenow", "40"));

  first.resolve("First file result");
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(2));
  const secondCall = ipc.callsTo("transcribe_file")[1];
  expect(secondCall.filePath).toBe("/audio/two.wav");
  expect(secondCall.runId).not.toBe(firstCall.runId);
  await waitFor(() => expect(ipc.listenerCount("plain-transcription-progress")).toBe(1));

  const secondTab = screen.getAllByRole("tab").find((tab) => tab.title === "/audio/two.wav");
  expect(secondTab).toBeDefined();
  await user.click(secondTab!);
  ipc.emit("plain-transcription-progress", { runId: firstCall.runId!, phase: "transcribing", percent: 92 });
  await tick();
  expect(screen.getByRole("progressbar", { name: "Read audio" })).toHaveAttribute("aria-valuetext", "Decoding the file");
  ipc.emit("plain-transcription-progress", { runId: secondCall.runId!, phase: "resampling", percent: 60 });
  await waitFor(() => expect(screen.getByRole("progressbar", { name: "Prepare audio & model" })).toHaveAttribute("aria-valuenow", "60"));

  second.resolve("Second file result");
  expect(await screen.findByDisplayValue("Second file result")).toBeInTheDocument();
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(3));
  const duplicateCall = ipc.callsTo("transcribe_file")[2];
  expect(duplicateCall.filePath).toBe("/audio/one.wav");
  expect(duplicateCall.runId).not.toBe(firstCall.runId);
  expect(duplicateCall.runId).not.toBe(secondCall.runId);
  const duplicateTab = screen
    .getAllByRole("tab")
    .filter((tab) => tab.title === "/audio/one.wav")
    .at(-1);
  expect(duplicateTab).toBeDefined();
  await user.click(duplicateTab!);
  duplicate.resolve("Duplicate file result");
  expect(await screen.findByDisplayValue("Duplicate file result")).toBeInTheDocument();
  await waitFor(() => expect(ipc.listenerCount("plain-transcription-progress")).toBe(0));
  expect(queueSummary()).toHaveTextContent(/^3 completed · 0 failed · 0 cancelled/);
});

test("Stop keeps a file running until native settlement and a native success wins the race", async () => {
  const transcription = deferred<string>();
  const cancellation = deferred<boolean>();
  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: () => transcription.promise,
      cancel_file_transcription: () => cancellation.promise,
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await openTranscribe(user);
  await drop(["/audio/race.wav"]);
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(1));
  const runId = ipc.callsTo("transcribe_file")[0].runId;

  await user.click(await screen.findByRole("button", { name: "Stop transcription" }));
  await waitFor(() => expect(ipc.callsTo("cancel_file_transcription")).toEqual([{ runId }]));
  cancellation.resolve(false);
  await waitFor(() => expect(screen.getByRole("button", { name: "Cancelling…" })).toBeDisabled());
  expect(queueSummary()).toHaveTextContent(/1 running/);

  transcription.resolve("Native success");
  expect(await screen.findByDisplayValue("Native success")).toBeInTheDocument();
  expect(await screen.findByText("Finished before Stop took effect.")).toBeInTheDocument();
  await waitFor(() => expect(queueSummary()).toHaveTextContent(/^1 completed · 0 failed · 0 cancelled/));
  expect(screen.queryByRole("alert")).toBeNull();
});

test("a late cancellation rejection cannot change a result that already settled", async () => {
  const transcription = deferred<string>();
  const cancellation = deferred<boolean>();
  let cancellationSettled = false;
  const cancellationReply = cancellation.promise.finally(() => {
    cancellationSettled = true;
  });
  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: () => transcription.promise,
      cancel_file_transcription: () => cancellationReply,
      copy_transcription_text: () => undefined,
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await openTranscribe(user);
  await drop(["/audio/late-stop.wav"]);
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(1));
  await user.click(await screen.findByRole("button", { name: "Stop transcription" }));
  await waitFor(() => expect(ipc.callsTo("cancel_file_transcription")).toHaveLength(1));

  transcription.resolve("Completed before stop reply");
  expect(await screen.findByDisplayValue("Completed before stop reply")).toBeInTheDocument();
  await waitFor(() => expect(queueSummary()).toHaveTextContent(/^1 completed · 0 failed/));
  const panel = screen.getByRole("tabpanel");
  await user.click(within(panel).getByRole("button", { name: "Copy" }));
  expect(await within(panel).findByText("Copied to clipboard.")).toBeInTheDocument();

  cancellation.reject(backendError("model_busy", "Late cancellation failure", true));
  await waitFor(() => expect(cancellationSettled).toBe(true));
  await settle();
  await tick();

  expect(within(panel).getByDisplayValue("Completed before stop reply")).toBeInTheDocument();
  expect(within(panel).getByText("Copied to clipboard.")).toBeInTheDocument();
  expect(screen.queryByText("Late cancellation failure")).toBeNull();
  expect(screen.queryByRole("alert")).toBeNull();
  expect(queueSummary()).toHaveTextContent(/^1 completed · 0 failed/);
});

test("a failed Stop request is cleared when the native transcription later succeeds", async () => {
  const transcription = deferred<string>();
  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: () => transcription.promise,
      cancel_file_transcription: () => {
        throw backendError("model_busy", "Stop request unavailable", true);
      },
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await openTranscribe(user);
  await drop(["/audio/stop-error.wav"]);
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(1));
  await user.click(await screen.findByRole("button", { name: "Stop transcription" }));
  expect(await screen.findByText("Stop request unavailable")).toBeInTheDocument();
  expect(queueSummary()).toHaveTextContent(/1 running/);

  transcription.resolve("Native success after failed Stop");
  expect(await screen.findByDisplayValue("Native success after failed Stop")).toBeInTheDocument();
  await waitFor(() => expect(queueSummary()).toHaveTextContent(/^1 completed · 0 failed/));
  expect(screen.queryByText("Stop request unavailable")).toBeNull();
  expect(screen.queryByRole("alert")).toBeNull();
});

test("accepted cancellation exposes no result, and only a file_not_found code removes a recent path", async () => {
  const transcription = deferred<string>();
  const cancelled = await renderSettings(
    settingsBackend({
      transcribe_file: () => transcription.promise,
      cancel_file_transcription: () => true,
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await openTranscribe(user);
  await drop(["/audio/cancelled.wav"]);
  await waitFor(() => expect(cancelled.ipc.callsTo("transcribe_file")).toHaveLength(1));
  await user.click(await screen.findByRole("button", { name: "Stop transcription" }));
  await waitFor(() => expect(cancelled.ipc.callsTo("cancel_file_transcription")).toHaveLength(1));
  transcription.reject(commandErrors.cancelled);

  expect(await screen.findByText("Transcription was cancelled.")).toBeInTheDocument();
  await waitFor(() => expect(queueSummary()).toHaveTextContent(/^0 completed · 0 failed · 1 cancelled/));
  expect(within(screen.getByRole("tabpanel")).queryByDisplayValue(/.+/)).toBeNull();
  expect(cancelled.ipc.listenerCount("plain-transcription-progress")).toBe(0);
  cancelled.view.unmount();

  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: ({ filePath }) => {
        if (filePath === "/gone/missing.wav") throw commandErrors.file_not_found;
        if (filePath === "/different/message-only.wav") {
          throw backendError("model_busy", commandErrors.file_not_found.message, true);
        }
        return "A different file completed";
      },
      set_update_result_pending: () => undefined,
    }),
  );
  const missingUser = userEvent.setup();
  await openTranscribe(missingUser);
  await drop(["/gone/missing.wav", "/different/message-only.wav", "/safe/good.wav"]);
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(3));
  expect(await screen.findByDisplayValue("A different file completed")).toBeInTheDocument();
  await waitFor(() => expect(queueSummary()).toHaveTextContent(/^1 completed · 2 failed/));

  const recent = screen.getByRole("combobox", { name: "Recent files to re-run (last 5, this session)" }) as HTMLSelectElement;
  const recentPaths = Array.from(recent.options, (option) => option.value);
  expect(recentPaths).toContain("/different/message-only.wav");
  expect(recentPaths).toContain("/safe/good.wav");
  expect(recentPaths).not.toContain("/gone/missing.wav");
});

test("each result copies and saves its own text and source while the next file is running", async () => {
  const first = deferred<string>();
  const second = deferred<string>();
  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: ({ filePath }) => (filePath === "/first/same.wav" ? first.promise : second.promise),
      copy_transcription_text: () => undefined,
      save_transcription_text: () => true,
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await openTranscribe(user);
  await drop(["/first/same.wav", "/second/same.wav"]);
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(1));

  first.resolve("First text");
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(2));
  expect(await screen.findByDisplayValue("First text")).toBeInTheDocument();
  expect(queueSummary()).toHaveTextContent(/1 completed · 0 failed · 0 cancelled · 0 queued · 1 running/);
  await user.click(screen.getByRole("button", { name: "Save…" }));
  await waitFor(() =>
    expect(ipc.callsTo("save_transcription_text")).toEqual([{ text: "First text", fileName: "same.txt", directory: "/first" }]),
  );
  await user.click(screen.getByRole("button", { name: "Copy" }));
  await waitFor(() => expect(ipc.callsTo("copy_transcription_text")).toEqual([{ text: "First text" }]));
  expect(ipc.callsTo("transcribe_file")).toHaveLength(2);

  const secondTab = screen.getAllByRole("tab").find((tab) => tab.title === "/second/same.wav");
  expect(secondTab).toBeDefined();
  await user.click(secondTab!);
  second.resolve("Second text");
  expect(await screen.findByDisplayValue("Second text")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Save…" }));
  await waitFor(() =>
    expect(ipc.callsTo("save_transcription_text")).toEqual([
      { text: "First text", fileName: "same.txt", directory: "/first" },
      { text: "Second text", fileName: "same.txt", directory: "/second" },
    ]),
  );
  expect(queueSummary()).toHaveTextContent(/^2 completed · 0 failed · 0 cancelled/);
});

test("Re-run uses the saved request after the current form changes", async () => {
  const firstBegin = deferred<string>();
  let beginCount = 0;
  const { ipc } = await renderSettings(
    settingsBackend({
      get_settings: () => multiProfileSettings(),
      begin_meeting_file: () => (++beginCount === 1 ? firstBegin.promise : "meeting-rerun"),
      get_meeting_job: ({ jobId }) => ({
        id: jobId,
        status: "failed",
        phase: "failed",
        error: "Synthetic meeting failure",
        transcript: null,
      }),
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await openTranscribe(user);
  await user.selectOptions(screen.getByRole("combobox", { name: "Profile (optional)" }), "english");
  await configurePlainFile(user, "Saved source context");
  await user.click(screen.getByRole("checkbox", { name: "Speaker diarization" }));
  await drop(["/chosen.wav"]);
  await waitFor(() => expect(ipc.callsTo("begin_meeting_file")).toHaveLength(1));
  expect(ipc.callsTo("begin_meeting_file")[0]).toEqual({
    filePath: "/chosen.wav",
    prompt: "Saved source context",
    profileId: "english",
  });
  expect(ipc.callsTo("transcribe_file")).toHaveLength(0);

  firstBegin.reject(backendError("model_busy", "Meeting import is busy", true));
  await waitFor(() => expect(queueSummary()).toHaveTextContent(/^0 completed · 1 failed/));

  // Re-run must use the source job's saved request, not these newly selected
  // form values. There is one recent entry, so this stays on the rendered control.
  await user.selectOptions(screen.getByRole("combobox", { name: "Profile (optional)" }), "default");
  const prompt = screen.getByRole("textbox", { name: "Extra context for this file" });
  await user.clear(prompt);
  await user.type(prompt, "Current form context");
  await user.click(screen.getByRole("checkbox", { name: "Speaker diarization" }));
  const recent = screen.getByRole("combobox", { name: "Recent files to re-run (last 5, this session)" });
  await waitFor(() => expect(recent).toHaveValue("/chosen.wav"));
  expect((recent as HTMLSelectElement).value).toBe("/chosen.wav");
  await user.click(screen.getByRole("button", { name: "Re-run" }));
  await waitFor(() => expect(ipc.callsTo("begin_meeting_file")).toHaveLength(2));
  expect(ipc.callsTo("begin_meeting_file")).toEqual([
    { filePath: "/chosen.wav", prompt: "Saved source context", profileId: "english" },
    { filePath: "/chosen.wav", prompt: "Saved source context", profileId: "english" },
  ]);
  expect(ipc.callsTo("transcribe_file")).toHaveLength(0);
  await waitFor(() => expect(ipc.callsTo("get_meeting_job")).toEqual([{ jobId: "meeting-rerun" }]));
});

test("Try again preserves a nondefault saved profile and context while another file waits", async () => {
  const first = deferred<string>();
  const originalRetry = deferred<string>();
  const third = deferred<string>();
  const retryAgain = deferred<string>();
  let retryCount = 0;
  const { ipc } = await renderSettings(
    settingsBackend({
      get_settings: () => multiProfileSettings(),
      transcribe_file: ({ filePath }) => {
        if (filePath === "/queue/first.wav") return first.promise;
        if (filePath === "/queue/retry.wav") return ++retryCount === 1 ? originalRetry.promise : retryAgain.promise;
        return third.promise;
      },
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await openTranscribe(user);
  await user.selectOptions(screen.getByRole("combobox", { name: "Profile (optional)" }), "english");
  await configurePlainFile(user, "Queue context");
  await drop(["/queue/first.wav", "/queue/retry.wav", "/queue/third.wav"]);
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(1));

  first.resolve("First result");
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(2));
  originalRetry.reject(commandErrors.model_busy);
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(3));

  const retryTab = screen.getAllByRole("tab").find((tab) => tab.title === "/queue/retry.wav");
  expect(retryTab).toBeDefined();
  await user.click(retryTab!);
  await user.click(await screen.findByRole("button", { name: "Try again" }));
  expect(ipc.callsTo("transcribe_file")).toHaveLength(3);
  expect(queueSummary()).toHaveTextContent(/1 queued · 1 running/);

  third.resolve("Third result");
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(4));
  const calls = ipc.callsTo("transcribe_file");
  expect(calls[3]).toMatchObject({
    filePath: calls[1].filePath,
    prompt: "Queue context",
    profileId: "english",
    diarize: false,
    autoPaste: false,
  });
  expect(calls[3].filePath).toBe("/queue/retry.wav");
  expect(calls[3].runId).not.toBe(calls[1].runId);
  retryAgain.resolve("Retried result");
  expect(await screen.findByDisplayValue("Retried result")).toBeInTheDocument();
  expect(within(screen.getByRole("tablist", { name: "Transcription results" })).getAllByRole("tab")).toHaveLength(4);
});

test("Try again uses the failed job's saved options after the form changes", async () => {
  let calls = 0;
  const { ipc } = await renderSettings(
    settingsBackend({
      get_settings: () => multiProfileSettings(),
      transcribe_file: () => {
        calls += 1;
        if (calls === 1) throw commandErrors.model_busy;
        return "Retried with saved options";
      },
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await openTranscribe(user);
  await user.selectOptions(screen.getByRole("combobox", { name: "Profile (optional)" }), "english");
  await configurePlainFile(user, "Saved retry context");
  await drop(["/saved/retry.wav"]);
  expect(await screen.findByRole("button", { name: "Try again" })).toBeInTheDocument();

  await user.selectOptions(screen.getByRole("combobox", { name: "Profile (optional)" }), "default");
  const prompt = screen.getByRole("textbox", { name: "Extra context for this file" });
  await user.clear(prompt);
  await user.type(prompt, "Current form context");
  await user.click(screen.getByRole("checkbox", { name: "Speaker diarization" }));

  await user.click(screen.getByRole("button", { name: "Try again" }));
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(2));
  const [failed, retried] = ipc.callsTo("transcribe_file");
  expect(failed).toMatchObject({ profileId: "english", prompt: "Saved retry context", diarize: false });
  expect(retried).toMatchObject({
    filePath: "/saved/retry.wav",
    profileId: "english",
    prompt: "Saved retry context",
    diarize: false,
    autoPaste: false,
  });
  expect(retried.runId).not.toBe(failed.runId);
  expect(await screen.findByDisplayValue("Retried with saved options")).toBeInTheDocument();
  expect(ipc.callsTo("begin_meeting_file")).toHaveLength(0);
  expect(within(screen.getByRole("tablist", { name: "Transcription results" })).getAllByRole("tab")).toHaveLength(2);
});
