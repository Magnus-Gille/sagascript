// Update recovery in Settings.svelte (#361): unsaved dictation, file and
// meeting results survive an updater restart. Settings is rendered against
// the typed IPC mock with an in-memory update-recovery.json; the tests drive
// it through the updater events and the Copy/Save/Discard controls, and
// assert on what is stored, what the updater is told and what the window
// shows. Replaces the source-text checks of scripts/test-settings-recovery.mjs;
// the mapping is in docs/testing/frontend.md.
import { fireEvent, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test, vi } from "vitest";
import {
  createUpdateRecoveryPayload,
  type UpdateRecoveryFile,
  type UpdateRecoveryMeeting,
  type UpdateRecoveryPayload,
} from "./update-recovery";
import type { MeetingTranscript } from "./meeting-types";
import { PIANISSIMO_ID } from "./models.svelte";
import { deferred, settle } from "../test/deferred";
import type { CommandHandlers, IpcMock } from "../test/ipc";
import { renderSettings, settingsBackend } from "../test/settings";

/** The `update-aborted` payload Rust sends: the error message (events.rs). */
const UPDATE_FAILED = "The update could not be installed";

const transcript: MeetingTranscript = {
  schema_version: 1,
  source_sha256: "meeting-source",
  language: "sv",
  model: PIANISSIMO_ID,
  duration_seconds: 4,
  segments: [{ id: "seg-1", start: 0, end: 4, text: "Välkomna till mötet", speaker: "s1" }],
  speakers: [{ id: "s1", label: "Talare 1" }],
};

const fileEntry: UpdateRecoveryFile = { job_id: "file-1", path: "/audio/interview.wav", text: "Intervjun i text" };
const otherFileEntry: UpdateRecoveryFile = { job_id: "file-2", path: "/audio/lecture.wav", text: "Föreläsningen i text" };
const meetingEntry: UpdateRecoveryMeeting = {
  job_id: "meeting-1",
  path: "/audio/meeting.wav",
  review: {
    review: { schema_version: 1, original: transcript, original_revision: "original-1", generation: 1, batches: [], revision: "review-1" },
    transcript,
  },
  // The draft MeetingReview reports for an unedited review: every field at its saved value.
  editor_draft: { labels: { s1: "Talare 1" }, mergeTargets: {}, texts: { "seg-1": "Välkomna till mötet" }, speakers: { "seg-1": "s1" } },
  proposal: null,
};

/** A stored update-recovery.json as Rust returns it. */
function stored(parts: Partial<Pick<UpdateRecoveryPayload, "dictation" | "files" | "meetings">>): UpdateRecoveryPayload {
  return createUpdateRecoveryPayload({ dictation: null, files: [], meetings: [], ...parts }, "2026-10-13T08:00:00.000Z");
}

/** What a fresh snapshot is expected to hold (its timestamp is new). */
function snapshot(parts: Partial<Pick<UpdateRecoveryPayload, "dictation" | "files" | "meetings">>) {
  return { ...stored(parts), saved_at: expect.any(String) };
}

/** update-recovery.json in memory: load, save and clear act on `current`. */
interface RecoveryFile {
  current: UpdateRecoveryPayload | null;
  handlers: CommandHandlers;
}

function recoveryFile(initial: UpdateRecoveryPayload | null = null): RecoveryFile {
  const file: RecoveryFile = {
    current: initial,
    handlers: {
      load_update_recovery: () => file.current,
      save_update_recovery: ({ payload }) => {
        file.current = payload;
      },
      clear_update_recovery: () => {
        file.current = null;
      },
    },
  };
  return file;
}

/** Settings with update recovery: the stored drafts plus the result and updater commands. */
function backend(file: RecoveryFile, overrides: CommandHandlers = {}): CommandHandlers {
  return settingsBackend({
    ...file.handlers,
    complete_update_preparation: () => undefined,
    copy_transcription_text: () => undefined,
    save_transcription_text: () => true,
    set_update_result_pending: () => undefined,
    acknowledge_update_result: () => true,
    save_meeting_review: () => true,
    ...overrides,
  });
}

const editor = () => screen.getByPlaceholderText("Click here and use your hotkey, or press the button above");

/** The backend finished a dictation the editor has not shown yet. */
function nativeResult(ipc: IpcMock, text: string, { pending = true } = {}): void {
  ipc.handle("get_last_transcription", () => text);
  ipc.handle("get_update_result_pending", ({ resultId }) => pending && resultId === "live-dictation");
}

/** The updater asks Settings to save its drafts; resolves with what Settings told it. */
async function prepareForUpdate(ipc: IpcMock, nonce: string) {
  ipc.emit("update-preparing", nonce);
  await waitFor(() => expect(ipc.callsTo("complete_update_preparation").map((call) => call.nonce)).toContain(nonce));
  return ipc.callsTo("complete_update_preparation").find((call) => call.nonce === nonce)!;
}

/** Commands invoked after the first `from` calls. */
function commandsSince(ipc: IpcMock, from: number): string[] {
  return ipc.calls.slice(from).map((call) => call.command);
}

function filePanel(name: string) {
  return screen.getByRole("tabpanel", { name: new RegExp(`^${name}`) });
}

async function showResult(user: ReturnType<typeof userEvent.setup>, name: string) {
  await user.click(screen.getByRole("button", { name: "Transcribe" }));
  await user.click(screen.getByRole("tab", { name: new RegExp(`^${name}`) }));
  return filePanel(name);
}

describe("restoring drafts after an update", () => {
  test("every stored draft comes back as a completed, reviewable result that is not transcribed or pasted again", async () => {
    const file = recoveryFile(stored({ dictation: { text: "Osparad diktering" }, files: [fileEntry], meetings: [meetingEntry] }));
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();

    const notice = await screen.findByRole("status", { name: "Recovered drafts" });
    expect(notice).toHaveTextContent("Recovered drafts Unsaved dictation and transcription results were restored after the update.");
    expect(editor()).toHaveValue("Osparad diktering");

    await user.click(screen.getByRole("button", { name: "Transcribe" }));
    const tabs = within(screen.getByRole("tablist", { name: "Transcription results" })).getAllByRole("tab");
    for (const tab of tabs) expect(within(tab).getByText("completed")).toBeInTheDocument();
    expect(tabs.map((tab) => tab.title)).toEqual(["/audio/interview.wav", "/audio/meeting.wav"]);
    expect(screen.getByRole("status", { name: "File transcription status" })).toHaveTextContent(/^2 completed · 0 failed/);
    // The first restored result is open, with its text and Copy/Save actions.
    expect(within(filePanel("interview.wav")).getByDisplayValue("Intervjun i text")).toBeInTheDocument();
    expect(within(filePanel("interview.wav")).getByRole("button", { name: "Copy" })).toBeEnabled();
    // The meeting comes back as its review, ready to save.
    const meeting = await showResult(user, "meeting.wav");
    expect(within(meeting).getByRole("button", { name: "Save review" })).toBeInTheDocument();
    expect(within(meeting).getAllByDisplayValue("Välkomna till mötet").length).toBeGreaterThan(0);

    // Nothing was transcribed, imported, pasted or delivered again.
    const work = ["transcribe_file", "begin_meeting_file", "get_meeting_job", "create_meeting_review", "open_meeting_review"];
    const delivery = ["copy_transcription_text", "save_transcription_text", "set_update_result_pending", "start_recording"];
    expect(ipc.calls.map((call) => call.command).filter((command) => [...work, ...delivery].includes(command))).toEqual([]);
    expect(file.current).toEqual(stored({ dictation: { text: "Osparad diktering" }, files: [fileEntry], meetings: [meetingEntry] }));
  });

  test("Discard recovered drafts deletes the stored drafts and removes them from the window", async () => {
    const file = recoveryFile(stored({ dictation: { text: "Osparad diktering" }, files: [fileEntry], meetings: [meetingEntry] }));
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Discard recovered drafts" }));
    await waitFor(() => expect(screen.queryByRole("status", { name: "Recovered drafts" })).toBeNull());
    expect(ipc.callsTo("clear_update_recovery")).toEqual([{}]);
    expect(file.current).toBeNull();
    expect(editor()).toHaveValue("");
    await user.click(screen.getByRole("button", { name: "Transcribe" }));
    expect(screen.queryByRole("tablist", { name: "Transcription results" })).toBeNull();
  });

  test("an unreadable recovery file blocks the update and keeps the drafts", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const file = recoveryFile();
    let reads = 0;
    const { ipc } = await renderSettings(
      backend(file, {
        // Only the startup read fails: the update stays blocked even if the
        // file reads fine later.
        load_update_recovery: () => {
          if (++reads === 1) throw "invalid JSON at line 1";
          return file.current;
        },
      }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Update blocked: unreadable recovery drafts were retained. invalid JSON at line 1. Back up or repair update-recovery.json in Sagascript's app data folder, then restart Sagascript before retrying.",
    );
    expect(warn).toHaveBeenCalledWith("Could not restore update recovery drafts", "invalid JSON at line 1");

    // The failed read stays a failure for the updater (and is never an
    // unhandled rejection, which would fail this test run).
    expect(await prepareForUpdate(ipc, "nonce-1")).toEqual({ nonce: "nonce-1", error: "invalid JSON at line 1" });
    expect(ipc.callsTo("save_update_recovery")).toEqual([]);
    expect(ipc.callsTo("clear_update_recovery")).toEqual([]);
  });
});

describe("preparing for an update", () => {
  test("the snapshot holds the open dictation, file and meeting results, then acknowledges the nonce", async () => {
    const file = recoveryFile(stored({ files: [fileEntry], meetings: [meetingEntry] }));
    const { ipc } = await renderSettings(backend(file));
    nativeResult(ipc, "Ny diktering");
    ipc.emit("transcription-result", "Ny diktering");
    await waitFor(() => expect(editor()).toHaveValue("Ny diktering"));

    const before = ipc.calls.length;
    expect(await prepareForUpdate(ipc, "nonce-1")).toEqual({ nonce: "nonce-1", error: null });
    expect(file.current).toEqual(snapshot({ dictation: { text: "Ny diktering" }, files: [fileEntry], meetings: [meetingEntry] }));
    // The native result is read before the editor is captured; the stored
    // drafts are reread just before the write.
    expect(commandsSince(ipc, before)).toEqual([
      "get_last_transcription",
      "get_update_result_pending",
      "get_update_result_pending",
      "load_update_recovery",
      "save_update_recovery",
      "complete_update_preparation",
    ]);
    expect(ipc.callsTo("get_update_result_pending").slice(-2)).toEqual([{ resultId: "live-dictation" }, { resultId: "live-dictation" }]);
    // The window stays read-only until the updater restarts or aborts.
    expect(editor()).toBeDisabled();
  });

  test("the snapshot waits for the startup restore, so restored drafts are part of it", async () => {
    const file = recoveryFile(stored({ files: [fileEntry] }));
    const restore = deferred<UpdateRecoveryPayload | null>();
    let reads = 0;
    const { ipc } = await renderSettings(backend(file, { load_update_recovery: () => (++reads === 1 ? restore.promise : file.current) }), {
      restored: false,
    });

    ipc.emit("update-preparing", "nonce-1");
    await settle();
    expect(ipc.callsTo("get_last_transcription")).toEqual([]);
    expect(ipc.callsTo("save_update_recovery")).toEqual([]);

    restore.resolve(file.current);
    await waitFor(() => expect(ipc.callsTo("complete_update_preparation")).toEqual([{ nonce: "nonce-1", error: null }]));
    expect(file.current).toEqual(snapshot({ files: [fileEntry] }));
  });

  describe("which dictation the snapshot keeps", () => {
    test("text typed into the editor", async () => {
      const file = recoveryFile();
      const { ipc } = await renderSettings(backend(file));
      await userEvent.setup().type(editor(), "Egen anteckning");
      await prepareForUpdate(ipc, "nonce-1");
      expect(file.current).toEqual(snapshot({ dictation: { text: "Egen anteckning" } }));
    });

    test("a recovered draft that is still undelivered", async () => {
      const file = recoveryFile(stored({ dictation: { text: "Återställd" } }));
      const { ipc } = await renderSettings(backend(file));
      await prepareForUpdate(ipc, "nonce-1");
      expect(file.current).toEqual(snapshot({ dictation: { text: "Återställd" } }));
    });

    test("not a native result the backend already delivered (for example pasted)", async () => {
      const file = recoveryFile();
      const { ipc } = await renderSettings(backend(file));
      nativeResult(ipc, "Inklistrad", { pending: false });
      ipc.emit("transcription-result", "Inklistrad");
      await waitFor(() => expect(editor()).toHaveValue("Inklistrad"));
      await prepareForUpdate(ipc, "nonce-1");
      expect(file.current).toEqual(snapshot({}));
    });

    test("not a native result the user already copied, even while the backend still reports it pending", async () => {
      const file = recoveryFile();
      const { ipc } = await renderSettings(backend(file));
      nativeResult(ipc, "Från diktering");
      ipc.emit("transcription-result", "Från diktering");
      await waitFor(() => expect(editor()).toHaveValue("Från diktering"));
      await userEvent.setup().click(screen.getByRole("button", { name: "Copy result" }));
      expect(await screen.findByText("Copied to clipboard.")).toBeInTheDocument();
      expect(ipc.callsTo("acknowledge_update_result")).toEqual([{ expectedText: "Från diktering" }]);

      await prepareForUpdate(ipc, "nonce-1");
      expect(file.current).toEqual(snapshot({}));
    });

    test("not a result the user already copied", async () => {
      const file = recoveryFile();
      const { ipc } = await renderSettings(backend(file));
      const user = userEvent.setup();
      await user.type(editor(), "Egen anteckning");
      await user.click(screen.getByRole("button", { name: "Copy result" }));
      expect(await screen.findByText("Copied to clipboard.")).toBeInTheDocument();
      await prepareForUpdate(ipc, "nonce-1");
      expect(file.current).toEqual(snapshot({}));
      expect(editor()).toHaveValue("Egen anteckning");
    });
  });

  test("a snapshot over the recovery limits fails the preparation with the reason and stores nothing", async () => {
    const file = recoveryFile(stored({ files: [fileEntry] }));
    const { ipc } = await renderSettings(backend(file));
    await fireEvent.input(editor(), { target: { value: "a".repeat(500_001) } });

    expect(await prepareForUpdate(ipc, "nonce-1")).toEqual({
      nonce: "nonce-1",
      error: "Unsaved results exceed the updater recovery limits. Save or remove some results before updating.",
    });
    expect(ipc.callsTo("save_update_recovery")).toEqual([]);
    expect(file.current).toEqual(stored({ files: [fileEntry] }));
    // A failed preparation hands the window back.
    await waitFor(() => expect(editor()).toBeEnabled());
  });

  test("the serialized snapshot is size-checked before it is handed to Rust", async () => {
    // 14 results of 500 000 three-byte characters each: every entry is within
    // its own limits, the serialized payload (21 MB) is not.
    const large = Array.from({ length: 14 }, (_, index) => ({
      job_id: `file-${index}`,
      path: `/audio/part-${index}.wav`,
      text: "€".repeat(500_000),
    }));
    const file = recoveryFile(stored({ files: large }));
    const { ipc } = await renderSettings(backend(file));
    expect(await prepareForUpdate(ipc, "nonce-1")).toEqual({
      nonce: "nonce-1",
      error: "Updater recovery payload exceeds the maximum size. Save or remove some results before updating.",
    });
    expect(ipc.callsTo("save_update_recovery")).toEqual([]);
  });

  test("a native result still in transit is delivered to the editor before the snapshot", async () => {
    const file = recoveryFile();
    const { ipc } = await renderSettings(backend(file));
    // The backend has finished "Sen diktering", but its event has not arrived.
    nativeResult(ipc, "Sen diktering");

    ipc.emit("update-preparing", "nonce-1");
    await settle(150);
    expect(ipc.callsTo("save_update_recovery")).toEqual([]);
    expect(ipc.callsTo("complete_update_preparation")).toEqual([]);

    ipc.emit("transcription-result", "Sen diktering");
    await waitFor(() => expect(ipc.callsTo("complete_update_preparation")).toEqual([{ nonce: "nonce-1", error: null }]));
    expect(file.current).toEqual(snapshot({ dictation: { text: "Sen diktering" } }));
  });

  test("a pending native result the editor never showed blocks the update", async () => {
    const file = recoveryFile(stored({ dictation: { text: "Återställd" } }));
    const handlers = backend(file, { get_last_transcription: () => "Ny diktering", get_update_result_pending: () => true });
    const { ipc } = await renderSettings(handlers);
    // The recovered draft keeps the editor; the newer native result is not shown.
    expect(editor()).toHaveValue("Återställd");

    expect(await prepareForUpdate(ipc, "nonce-1")).toEqual({
      nonce: "nonce-1",
      error: "A different unsaved dictation is already open. Copy or save both results before retrying the update.",
    });
    expect(file.current).toEqual(stored({ dictation: { text: "Återställd" } }));
  });

  test("a native result the user cleared from the editor does not block the update", async () => {
    const file = recoveryFile();
    const { ipc } = await renderSettings(backend(file));
    nativeResult(ipc, "Ny diktering");
    ipc.emit("transcription-result", "Ny diktering");
    await waitFor(() => expect(editor()).toHaveValue("Ny diktering"));
    await userEvent.setup().clear(editor());

    expect(await prepareForUpdate(ipc, "nonce-1")).toEqual({ nonce: "nonce-1", error: null });
    expect(file.current).toEqual(snapshot({}));
  });

  test("an aborted or superseded preparation never stores or acknowledges anything", async () => {
    const file = recoveryFile();
    const { ipc } = await renderSettings(backend(file));
    await userEvent.setup().type(editor(), "Egen anteckning");
    const nativeRead = deferred<string | null>();
    ipc.handle("get_last_transcription", () => nativeRead.promise);

    // Aborted while it waits for the backend.
    ipc.emit("update-preparing", "nonce-1");
    await waitFor(() => expect(editor()).toBeDisabled());
    ipc.emit("update-aborted", UPDATE_FAILED);
    await waitFor(() => expect(editor()).toBeEnabled());
    nativeRead.resolve(null);
    await settle();
    expect(ipc.callsTo("save_update_recovery")).toEqual([]);
    expect(ipc.callsTo("complete_update_preparation")).toEqual([]);

    // Superseded by the next request: only the newest nonce is answered.
    const secondRead = deferred<string | null>();
    ipc.handle("get_last_transcription", () => secondRead.promise);
    ipc.emit("update-preparing", "nonce-2");
    ipc.emit("update-preparing", "nonce-3");
    secondRead.resolve(null);
    await waitFor(() => expect(ipc.callsTo("complete_update_preparation")).toEqual([{ nonce: "nonce-3", error: null }]));
    await settle();
    expect(ipc.callsTo("complete_update_preparation")).toEqual([{ nonce: "nonce-3", error: null }]);
    expect(ipc.callsTo("save_update_recovery")).toHaveLength(1);
    expect(file.current).toEqual(snapshot({ dictation: { text: "Egen anteckning" } }));
  });

  test("a preparation aborted while it waits behind another recovery write does nothing when its turn comes", async () => {
    const file = recoveryFile(stored({ files: [fileEntry, otherFileEntry] }));
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();
    const reread = deferred<UpdateRecoveryPayload | null>();
    ipc.handle("load_update_recovery", () => reread.promise);
    const interview = await showResult(user, "interview.wav");
    await user.click(within(interview).getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(ipc.callsTo("load_update_recovery")).toHaveLength(2));

    ipc.emit("update-preparing", "nonce-1");
    await waitFor(() => expect(ipc.callsTo("get_update_result_pending")).toHaveLength(1));
    ipc.emit("update-aborted", UPDATE_FAILED);
    const before = ipc.calls.length;
    ipc.handle("load_update_recovery", () => file.current);
    reread.resolve(file.current);
    await waitFor(() => expect(file.current).toEqual(stored({ files: [otherFileEntry] })));
    await settle();
    // Only the cleanup's own write: the aborted preparation never reads,
    // stores or acknowledges anything.
    expect(commandsSince(ipc, before)).toEqual(["save_update_recovery"]);
  });

  test("a preparation aborted while its snapshot is being written is not acknowledged", async () => {
    const file = recoveryFile();
    const { ipc } = await renderSettings(backend(file));
    await userEvent.setup().type(editor(), "Egen anteckning");
    const write = deferred<void>();
    ipc.handle("save_update_recovery", () => write.promise);

    ipc.emit("update-preparing", "nonce-1");
    await waitFor(() => expect(ipc.callsTo("save_update_recovery")).toHaveLength(1));
    ipc.emit("update-aborted", UPDATE_FAILED);
    write.resolve();
    await settle();
    expect(ipc.callsTo("complete_update_preparation")).toEqual([]);
  });

  test("a superseded preparation that fails leaves the newer one in charge", async () => {
    const file = recoveryFile();
    const { ipc } = await renderSettings(backend(file));
    await userEvent.setup().type(editor(), "Egen anteckning");
    const firstWrite = deferred<void>();
    let writes = 0;
    ipc.handle("save_update_recovery", ({ payload }) => {
      if (++writes === 1) return firstWrite.promise;
      file.current = payload;
    });

    ipc.emit("update-preparing", "nonce-1");
    await waitFor(() => expect(ipc.callsTo("save_update_recovery")).toHaveLength(1));
    ipc.emit("update-preparing", "nonce-2");
    firstWrite.reject("disk full");
    await waitFor(() => expect(ipc.callsTo("complete_update_preparation")).toEqual([{ nonce: "nonce-2", error: null }]));
    await settle();
    expect(ipc.callsTo("complete_update_preparation")).toEqual([{ nonce: "nonce-2", error: null }]);
    // The window stays read-only for the preparation that is still active.
    expect(editor()).toBeDisabled();
  });

  test("a failed write is reported for the active preparation, not after an abort", async () => {
    const file = recoveryFile();
    const { ipc } = await renderSettings(backend(file));
    await userEvent.setup().type(editor(), "Egen anteckning");

    ipc.handle("save_update_recovery", () => {
      throw "disk full";
    });
    expect(await prepareForUpdate(ipc, "nonce-1")).toEqual({ nonce: "nonce-1", error: "disk full" });
    await waitFor(() => expect(editor()).toBeEnabled());

    const write = deferred<void>();
    ipc.handle("save_update_recovery", () => write.promise);
    ipc.emit("update-preparing", "nonce-2");
    await waitFor(() => expect(ipc.callsTo("save_update_recovery")).toHaveLength(2));
    ipc.emit("update-aborted", UPDATE_FAILED);
    write.reject("disk full");
    await settle();
    expect(ipc.callsTo("complete_update_preparation")).toEqual([{ nonce: "nonce-1", error: "disk full" }]);
  });
});

describe("replacing a stored dictation", () => {
  test("a newer native result may not overwrite a recovered draft it did not come from", async () => {
    const file = recoveryFile(stored({ dictation: { text: "Återställd" } }));
    const { ipc } = await renderSettings(backend(file));
    nativeResult(ipc, "Ny diktering");
    ipc.emit("transcription-result", "Ny diktering");
    await waitFor(() => expect(editor()).toHaveValue("Ny diktering"));

    expect(await prepareForUpdate(ipc, "nonce-1")).toEqual({
      nonce: "nonce-1",
      error: "A different unsaved dictation is already in update recovery. Copy or save both results before retrying the update.",
    });
    expect(file.current).toEqual(stored({ dictation: { text: "Återställd" } }));
  });

  test("the editor that stored a dictation replaces it, and delivering a later edit removes it", async () => {
    const file = recoveryFile(stored({ dictation: { text: "Återställd" } }));
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();
    await user.type(editor(), " och ändrad");

    expect(await prepareForUpdate(ipc, "nonce-1")).toEqual({ nonce: "nonce-1", error: null });
    expect(file.current).toEqual(snapshot({ dictation: { text: "Återställd och ändrad" } }));

    ipc.emit("update-aborted", UPDATE_FAILED);
    await waitFor(() => expect(editor()).toBeEnabled());
    await user.type(editor(), " igen");
    await user.click(screen.getByRole("button", { name: "Copy result" }));
    await waitFor(() => expect(ipc.callsTo("clear_update_recovery")).toEqual([{}]));
    expect(ipc.callsTo("copy_transcription_text")).toEqual([{ text: "Återställd och ändrad igen" }]);
    expect(file.current).toBeNull();
  });
});

describe("cleaning up delivered drafts", () => {
  test("a stored draft is removed only by delivering it from the editor it was restored into", async () => {
    const file = recoveryFile(stored({ dictation: { text: "Hej" } }));
    const { ipc } = await renderSettings(backend(file));
    // A new dictation with the same text replaces the restored one.
    nativeResult(ipc, "Hej");
    ipc.emit("transcription-result", "Hej");
    await settle();

    await userEvent.setup().click(screen.getByRole("button", { name: "Copy result" }));
    expect(await screen.findByText("Copied to clipboard.")).toBeInTheDocument();
    await settle();
    expect(ipc.callsTo("clear_update_recovery")).toEqual([]);
    expect(file.current).toEqual(stored({ dictation: { text: "Hej" } }));
  });

  test("copying or saving a recovered result removes exactly that entry from the stored drafts", async () => {
    const file = recoveryFile(stored({ dictation: { text: "Osparad diktering" }, files: [fileEntry], meetings: [meetingEntry] }));
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();

    const interview = await showResult(user, "interview.wav");
    await user.click(within(interview).getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(file.current?.files).toEqual([]));
    expect(ipc.callsTo("copy_transcription_text")).toEqual([{ text: "Intervjun i text" }]);
    expect(ipc.callsTo("set_update_result_pending")).toEqual([{ resultId: "file-1", pending: false }]);
    expect(file.current).toEqual(stored({ dictation: { text: "Osparad diktering" }, meetings: [meetingEntry] }));

    const meeting = await showResult(user, "meeting.wav");
    await user.click(within(meeting).getByRole("button", { name: "Save review" }));
    await waitFor(() => expect(file.current?.meetings).toEqual([]));
    expect(ipc.callsTo("set_update_result_pending")).toContainEqual({ resultId: "meeting:meeting-1", pending: false });
    expect(file.current).toEqual(stored({ dictation: { text: "Osparad diktering" } }));
    expect(screen.getByRole("status", { name: "Recovered drafts" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Dictate" }));
    await user.click(screen.getByRole("button", { name: "Copy result" }));
    await waitFor(() => expect(file.current).toBeNull());
    expect(ipc.callsTo("clear_update_recovery")).toEqual([{}]);
    // Every recovered draft is delivered: the notice goes away.
    await waitFor(() => expect(screen.queryByRole("status", { name: "Recovered drafts" })).toBeNull());
  });

  test("cleanup rereads the stored drafts and never adds unsaved results to them", async () => {
    const file = recoveryFile(stored({ files: [fileEntry], meetings: [meetingEntry] }));
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();
    // The stored file is the source of truth: it changed since the restore.
    file.current = stored({ files: [fileEntry, otherFileEntry], meetings: [meetingEntry] });
    await user.type(editor(), "Inte sparad");

    const before = ipc.calls.length;
    const interview = await showResult(user, "interview.wav");
    await user.click(within(interview).getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(ipc.callsTo("save_update_recovery")).toHaveLength(1));
    expect(file.current).toEqual(stored({ files: [otherFileEntry], meetings: [meetingEntry] }));
    expect(commandsSince(ipc, before).filter((command) => command.includes("update_recovery"))).toEqual([
      "load_update_recovery",
      "save_update_recovery",
    ]);
  });

  test("a delivery that matches no stored draft writes nothing", async () => {
    const file = recoveryFile(stored({ files: [fileEntry] }));
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();
    await user.type(editor(), "Ny text");
    const reads = ipc.callsTo("load_update_recovery").length;

    await user.click(screen.getByRole("button", { name: "Copy result" }));
    await waitFor(() => expect(ipc.callsTo("load_update_recovery")).toHaveLength(reads + 1));
    await settle();
    expect(ipc.callsTo("save_update_recovery")).toEqual([]);
    expect(ipc.callsTo("clear_update_recovery")).toEqual([]);
    expect(file.current).toEqual(stored({ files: [fileEntry] }));
  });

  test("an edited recovered dictation still removes its stored original when delivered", async () => {
    const file = recoveryFile(stored({ dictation: { text: "Återställd" } }));
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();
    await user.type(editor(), ", rättad");
    await user.click(screen.getByRole("button", { name: "Save result…" }));
    expect(await screen.findByText("Saved.")).toBeInTheDocument();
    expect(ipc.callsTo("save_transcription_text")).toEqual([{ text: "Återställd, rättad", fileName: "dictation.txt", directory: null }]);
    await waitFor(() => expect(file.current).toBeNull());
  });

  test("delivering a native result acknowledges the native text it came from, even after edits", async () => {
    const file = recoveryFile();
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();
    nativeResult(ipc, "Från diktering");
    ipc.emit("transcription-result", "Från diktering");
    await waitFor(() => expect(editor()).toHaveValue("Från diktering"));
    await user.type(editor(), ", rättad");

    await user.click(screen.getByRole("button", { name: "Copy result" }));
    await waitFor(() => expect(ipc.callsTo("acknowledge_update_result")).toEqual([{ expectedText: "Från diktering" }]));
    expect(ipc.callsTo("copy_transcription_text")).toEqual([{ text: "Från diktering, rättad" }]);
  });

  test("a recovered draft that is also the pending native result is acknowledged when delivered", async () => {
    const file = recoveryFile(stored({ dictation: { text: "Samma text" } }));
    const { ipc } = await renderSettings(
      backend(file, { get_last_transcription: () => "Samma text", get_update_result_pending: () => true }),
    );

    await userEvent.setup().click(screen.getByRole("button", { name: "Copy result" }));
    await waitFor(() => expect(file.current).toBeNull());
    expect(ipc.callsTo("acknowledge_update_result")).toEqual([{ expectedText: "Samma text" }]);
  });

  test("a result that arrives during Copy stays open while the delivered draft is removed", async () => {
    const file = recoveryFile(stored({ dictation: { text: "Återställd" } }));
    const copy = deferred<void>();
    const { ipc } = await renderSettings(backend(file, { copy_transcription_text: () => copy.promise }));
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "Copy result" }));
    await waitFor(() => expect(ipc.callsTo("copy_transcription_text")).toHaveLength(1));
    nativeResult(ipc, "Ny diktering");
    ipc.emit("transcription-result", "Ny diktering");
    copy.resolve();

    await waitFor(() => expect(file.current).toBeNull());
    expect(await screen.findByText("Copied to clipboard.")).toBeInTheDocument();
    expect(editor()).toHaveValue("Ny diktering");
    expect(ipc.callsTo("acknowledge_update_result")).toEqual([]);
    // The new result is still undelivered and is saved by the next update.
    await prepareForUpdate(ipc, "nonce-1");
    expect(file.current).toEqual(snapshot({ dictation: { text: "Ny diktering" } }));
  });

  test("when cleanup cannot read the drafts, Copy still succeeds and the window names the repair path", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const repair =
      "Update recovery draft was retained after delivery. permission denied. Back up or repair update-recovery.json in Sagascript's app data folder, then restart Sagascript before retrying the update.";
    const file = recoveryFile(stored({ dictation: { text: "Återställd" }, files: [fileEntry] }));
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();
    ipc.handle("load_update_recovery", () => {
      throw "permission denied";
    });

    await user.click(screen.getByRole("button", { name: "Copy result" }));
    expect(await screen.findByText("Copied to clipboard.")).toBeInTheDocument();
    expect(await screen.findByRole("alert")).toHaveTextContent(repair);
    expect(warn).toHaveBeenCalledWith("Could not clear delivered update recovery draft", "permission denied");

    const interview = await showResult(user, "interview.wav");
    await user.click(within(interview).getByRole("button", { name: "Copy" }));
    expect(await within(interview).findByText("Copied to clipboard.")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(repair);

    expect(ipc.callsTo("save_update_recovery")).toEqual([]);
    expect(ipc.callsTo("clear_update_recovery")).toEqual([]);
    expect(file.current).toEqual(stored({ dictation: { text: "Återställd" }, files: [fileEntry] }));
  });

  test("cleanup, preparation and discard write the drafts one at a time, in request order", async () => {
    const file = recoveryFile(stored({ files: [fileEntry, otherFileEntry], meetings: [meetingEntry] }));
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();
    const reread = deferred<UpdateRecoveryPayload | null>();
    ipc.handle("load_update_recovery", () => reread.promise);

    // Copy starts a cleanup that waits for its reread...
    const interview = await showResult(user, "interview.wav");
    await user.click(within(interview).getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(ipc.callsTo("load_update_recovery")).toHaveLength(2));
    // ...so an update preparation queues behind it.
    const before = ipc.calls.length;
    ipc.emit("update-preparing", "nonce-1");
    await settle();
    expect(ipc.callsTo("load_update_recovery")).toHaveLength(2);
    expect(ipc.callsTo("save_update_recovery")).toEqual([]);

    ipc.handle("load_update_recovery", () => file.current);
    reread.resolve(file.current);
    await waitFor(() => expect(ipc.callsTo("complete_update_preparation")).toEqual([{ nonce: "nonce-1", error: null }]));
    expect(commandsSince(ipc, before).filter((command) => command.includes("update_recovery"))).toEqual([
      "save_update_recovery", // the cleanup: without file-1
      "load_update_recovery", // the preparation, after it
      "save_update_recovery",
    ]);
    expect(ipc.callsTo("save_update_recovery")[0]).toEqual({
      payload: stored({ files: [otherFileEntry], meetings: [meetingEntry] }),
    });
    expect(file.current).toEqual(snapshot({ files: [otherFileEntry], meetings: [meetingEntry] }));
  });

  test("Discard waits for a running cleanup instead of racing it", async () => {
    const file = recoveryFile(stored({ files: [fileEntry, otherFileEntry] }));
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();
    const reread = deferred<UpdateRecoveryPayload | null>();
    ipc.handle("load_update_recovery", () => reread.promise);

    const interview = await showResult(user, "interview.wav");
    await user.click(within(interview).getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(ipc.callsTo("load_update_recovery")).toHaveLength(2));
    await user.click(screen.getByRole("button", { name: "Discard recovered drafts" }));
    await settle();
    expect(ipc.callsTo("clear_update_recovery")).toEqual([]);

    reread.resolve(file.current);
    await waitFor(() => expect(ipc.callsTo("clear_update_recovery")).toEqual([{}]));
    const writes = ipc.calls
      .map((call) => call.command)
      .filter((command) => command === "save_update_recovery" || command === "clear_update_recovery");
    expect(writes).toEqual(["save_update_recovery", "clear_update_recovery"]);
    expect(file.current).toBeNull();
  });
});
