// UpdateRecoveryController (SAGA-379), driven directly: its write queue, its
// error paths and its ordering. The real api.ts wrappers run over the typed
// IPC mock with an in-memory update-recovery.json, so the tests assert the
// real commands in the order they reach the backend. Settings.recovery.test.ts
// covers the same flows through the rendered Settings window.
import { describe, expect, test, vi } from "vitest";
import * as api from "../api";
import type { MeetingTranscript } from "../meeting-types";
import { PIANISSIMO_ID } from "../models.svelte";
import type { FileJob } from "../transcription-queue";
import {
  createUpdateRecoveryPayload,
  type UpdateRecoveryFile,
  type UpdateRecoveryMeeting,
  type UpdateRecoveryPayload,
} from "../update-recovery";
import { UpdateRecoveryController, type DictationEditorPort, type FileQueuePort } from "./updateRecovery.svelte";
import { deferred, settle } from "../../test/deferred";
import { backendError, mockIpc, type CommandHandlers, type IpcMock } from "../../test/ipc";

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
  editor_draft: { labels: { s1: "Talare 1" }, mergeTargets: {}, texts: { "seg-1": "Välkomna till mötet" }, speakers: { "seg-1": "s1" } },
  proposal: null,
};

const DRAFT = "Osparad diktering";

/** A stored update-recovery.json as Rust returns it. */
function stored(parts: Partial<Pick<UpdateRecoveryPayload, "dictation" | "files" | "meetings">>): UpdateRecoveryPayload {
  return createUpdateRecoveryPayload({ dictation: null, files: [], meetings: [], ...parts }, "2026-10-13T08:00:00.000Z");
}

/** What a fresh snapshot is expected to hold (its timestamp is new). */
function snapshot(parts: Partial<Pick<UpdateRecoveryPayload, "dictation" | "files" | "meetings">>) {
  return { ...stored(parts), saved_at: expect.any(String) };
}

function completedJob(id: string, path: string): FileJob {
  return { id, path, diarize: false, prompt: null, profileId: null, status: "completed" };
}

const readyPanel = { prepareUpdateRecovery: () => Promise.resolve() };

interface HarnessOptions {
  initial?: UpdateRecoveryPayload | null;
  handlers?: CommandHandlers;
  dictation?: Partial<DictationEditorPort>;
  files?: Partial<FileQueuePort>;
  settle?: () => Promise<void>;
}

/** A controller over plain-object ports, the IPC mock and update-recovery.json in memory (`file.current`). */
function harness(options: HarnessOptions = {}) {
  const file = { current: options.initial ?? null };
  const ipc = mockIpc({
    load_update_recovery: () => file.current,
    save_update_recovery: ({ payload }) => {
      file.current = payload;
    },
    clear_update_recovery: () => {
      file.current = null;
    },
    complete_update_preparation: () => undefined,
    get_last_transcription: () => null,
    get_update_result_pending: () => false,
    acknowledge_update_result: () => true,
    ...options.handlers,
  });
  const dictation = { text: "", recoveryPending: false, edited: false, transcribing: false, ...options.dictation };
  const files: FileQueuePort = { jobs: [], savedReviewIds: [], selectedId: null, ...options.files };
  const discardFailures: string[] = [];
  const controller = new UpdateRecoveryController({
    api,
    dictation,
    files,
    onDiscardFailed: (message) => discardFailures.push(message),
    settle: options.settle,
  });
  return { file, ipc, dictation, files, discardFailures, controller };
}

/** Restore the stored drafts and wait until they are open. */
async function restoredHarness(options: HarnessOptions) {
  const h = harness(options);
  h.controller.restore();
  await h.controller.restored;
  return h;
}

function commandsSince(ipc: IpcMock, from: number): string[] {
  return ipc.calls.slice(from).map((call) => call.command);
}

/** Resolves with the updater's acknowledgement of `nonce`. */
async function acknowledged(ipc: IpcMock, nonce: string) {
  await vi.waitFor(() => expect(ipc.callsTo("complete_update_preparation").map((call) => call.nonce)).toContain(nonce));
  return ipc.callsTo("complete_update_preparation").find((call) => call.nonce === nonce);
}

describe("the write queue", () => {
  test("cleanup, discard and a snapshot write the file one at a time, in the order they were requested", async () => {
    const h = await restoredHarness({ initial: stored({ files: [fileEntry, otherFileEntry] }) });
    h.controller.setResultPanel("file-1", readyPanel);
    h.controller.setResultPanel("file-2", readyPanel);
    const heldRead = deferred<UpdateRecoveryPayload | null>();
    h.ipc.handle("load_update_recovery", () => heldRead.promise);
    const from = h.ipc.calls.length;

    h.controller.onFileRecoveryChange(null, "file-1"); // file-1 was delivered: its cleanup reads the file
    const discard = h.controller.discardDrafts();
    h.controller.beginPreparation("nonce-1");
    // The snapshot's reads outside the queue run, but nothing queued starts while the cleanup's read is held.
    await vi.waitFor(() => expect(h.ipc.callsTo("get_update_result_pending")).toHaveLength(1));
    await settle();
    expect(commandsSince(h.ipc, from)).toEqual(["load_update_recovery", "get_last_transcription", "get_update_result_pending"]);

    h.ipc.handle("load_update_recovery", () => h.file.current);
    heldRead.resolve(h.file.current);
    await discard;
    await acknowledged(h.ipc, "nonce-1");
    expect(commandsSince(h.ipc, from)).toEqual([
      "load_update_recovery",
      "get_last_transcription",
      "get_update_result_pending",
      "save_update_recovery", // the cleanup keeps file-2
      "clear_update_recovery", // the discard
      "get_update_result_pending", // the snapshot, inside the queue
      "load_update_recovery",
      "save_update_recovery",
      "complete_update_preparation",
    ]);
    expect(h.ipc.callsTo("save_update_recovery")).toEqual([{ payload: stored({ files: [otherFileEntry] }) }, { payload: snapshot({}) }]);
  });

  test("a mutation that fails does not block the next one", async () => {
    const h = await restoredHarness({ initial: stored({ files: [fileEntry] }) });
    h.ipc.handle("load_update_recovery", () => {
      throw backendError("io", "Could not read update-recovery.json.");
    });
    const from = h.ipc.calls.length;

    h.controller.onFileRecoveryChange(null, "file-1");
    await vi.waitFor(() => expect(h.controller.readError).toMatch(/^Update recovery draft was retained after delivery\./));
    await h.controller.discardDrafts();

    expect(commandsSince(h.ipc, from)).toEqual(["load_update_recovery", "clear_update_recovery"]);
    expect(h.discardFailures).toEqual([]);
    expect(h.controller.readError).toBe("");
  });

  test("a cleanup rereads the file and writes nothing when nothing in it matches", async () => {
    const h = await restoredHarness({ initial: stored({ files: [fileEntry] }) });
    h.file.current = stored({ files: [otherFileEntry] }); // the file changed since it was restored
    const from = h.ipc.calls.length;

    h.controller.onFileRecoveryChange(null, "file-1");
    await vi.waitFor(() => expect(commandsSince(h.ipc, from)).toEqual(["load_update_recovery"]));
    await settle();

    expect(commandsSince(h.ipc, from)).toEqual(["load_update_recovery"]);
    expect(h.file.current).toEqual(stored({ files: [otherFileEntry] }));
  });

  test("a cleanup that leaves other entries saves the remainder", async () => {
    const h = await restoredHarness({ initial: stored({ files: [fileEntry], meetings: [meetingEntry] }) });
    const from = h.ipc.calls.length;

    h.controller.onMeetingRecoveryChange(null, "meeting-1");
    await vi.waitFor(() => expect(commandsSince(h.ipc, from)).toEqual(["load_update_recovery", "save_update_recovery"]));

    expect(h.ipc.callsTo("save_update_recovery")).toEqual([{ payload: stored({ files: [fileEntry] }) }]);
    expect(h.controller.meetingEntries).toEqual([]);
    expect(h.controller.draftsNotice).toBe(true); // file-1 is still a restored draft
  });

  test("a cleanup that empties the file clears it instead of saving", async () => {
    const h = await restoredHarness({ initial: stored({ files: [fileEntry] }) });
    const from = h.ipc.calls.length;

    h.controller.onFileRecoveryChange(null, "file-1");
    await vi.waitFor(() => expect(commandsSince(h.ipc, from)).toEqual(["load_update_recovery", "clear_update_recovery"]));

    expect(h.file.current).toBeNull();
    expect(h.controller.draftsNotice).toBe(false);
  });
});

describe("restore", () => {
  test("stored dictation, files and meetings reopen as completed drafts", async () => {
    const h = await restoredHarness({
      initial: stored({ dictation: { text: DRAFT }, files: [fileEntry], meetings: [meetingEntry] }),
    });

    expect(h.dictation).toMatchObject({ text: DRAFT, recoveryPending: true, edited: false });
    expect(h.files.jobs).toEqual([completedJob("file-1", fileEntry.path), completedJob("meeting-1", meetingEntry.path)]);
    expect(h.files.savedReviewIds).toEqual(["meeting-1"]);
    expect(h.files.selectedId).toBe("file-1");
    expect(h.controller.fileEntries).toEqual([fileEntry]);
    expect(h.controller.meetingEntries).toEqual([meetingEntry]);
    expect(h.controller.draftsNotice).toBe(true);
    expect(h.controller.readError).toBe("");
  });

  test("restore keeps a non-empty editor, the current selection and jobs whose ids already exist", async () => {
    const running: FileJob = { ...completedJob("file-1", fileEntry.path), status: "running" };
    const h = await restoredHarness({
      initial: stored({ dictation: { text: DRAFT }, files: [fileEntry, otherFileEntry], meetings: [meetingEntry] }),
      dictation: { text: "Mitt eget utkast" },
      files: { jobs: [running], savedReviewIds: ["meeting-1"], selectedId: "file-1" },
    });

    expect(h.dictation).toMatchObject({ text: "Mitt eget utkast", recoveryPending: false });
    expect(h.files.jobs).toEqual([running, completedJob("file-2", otherFileEntry.path)]);
    expect(h.files.savedReviewIds).toEqual(["meeting-1"]);
    expect(h.files.selectedId).toBe("file-1");
    expect(h.controller.fileEntries).toEqual([otherFileEntry]);
    expect(h.controller.meetingEntries).toEqual([]);
  });

  test.each([
    ["a dictation started", (controller: UpdateRecoveryController) => controller.dictationStarted(), ""],
    ["a native result arrived", (controller: UpdateRecoveryController) => controller.showNativeResult("Ny diktering"), "Ny diktering"],
  ])("restore leaves the editor alone when %s while the file was being read", async (_name, interrupt, expectedText) => {
    const heldRead = deferred<UpdateRecoveryPayload | null>();
    const h = harness({ handlers: { load_update_recovery: () => heldRead.promise } });
    h.controller.restore();
    await vi.waitFor(() => expect(h.ipc.callsTo("load_update_recovery")).toHaveLength(1));

    interrupt(h.controller);
    heldRead.resolve(stored({ dictation: { text: DRAFT }, files: [fileEntry] }));
    await h.controller.restored;

    expect(h.dictation.text).toBe(expectedText);
    expect(h.files.jobs).toEqual([completedJob("file-1", fileEntry.path)]); // files still reopen
  });

  test("an unreadable file blocks the update with the backend's reason and saves nothing", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const h = harness({
      handlers: {
        load_update_recovery: () => {
          throw backendError("io", "Permission denied.");
        },
      },
    });

    h.controller.restore();
    await expect(h.controller.restored).rejects.toMatchObject({ message: "Permission denied." });
    expect(h.controller.readError).toMatch(/^Update blocked: unreadable recovery drafts were retained\. /);
    expect(h.controller.readError).toContain("Permission denied");
    expect(warn).toHaveBeenCalled();

    const from = h.ipc.calls.length;
    h.controller.beginPreparation("nonce-1");
    expect(await acknowledged(h.ipc, "nonce-1")).toEqual({ nonce: "nonce-1", error: "Permission denied." });
    expect(commandsSince(h.ipc, from)).toEqual(["complete_update_preparation"]);
    expect(h.controller.preparing).toBe(false);
  });
});

describe("the snapshot (beginPreparation)", () => {
  test("waits for the restore, asks every panel, drains, then saves and acknowledges inside the queue", async () => {
    const log: string[] = [];
    const heldRestore = deferred<UpdateRecoveryPayload | null>();
    const restoredFile = stored({ dictation: { text: DRAFT }, files: [fileEntry] });
    const h = harness({
      settle: async () => {
        log.push("settle");
      },
    });
    let current: UpdateRecoveryPayload | null = restoredFile;
    let first = true;
    h.ipc.handle("load_update_recovery", () => {
      log.push("load_update_recovery");
      if (!first) return current;
      first = false;
      return heldRestore.promise;
    });
    h.ipc.handle("save_update_recovery", ({ payload }) => {
      log.push("save_update_recovery");
      current = payload;
    });
    h.ipc.handle("get_last_transcription", () => {
      log.push("get_last_transcription");
      return null;
    });
    h.ipc.handle("get_update_result_pending", () => {
      log.push("get_update_result_pending");
      return false;
    });
    h.ipc.handle("complete_update_preparation", () => {
      log.push("complete_update_preparation");
    });
    h.controller.setResultPanel("file-1", {
      prepareUpdateRecovery: async () => {
        log.push("panel file-1");
      },
    });

    h.controller.restore();
    h.controller.beginPreparation("nonce-1");
    expect(h.controller.preparing).toBe(true);
    await settle();
    expect(log).toEqual(["load_update_recovery"]); // nothing happens before the restore finished

    heldRestore.resolve(restoredFile);
    expect(await acknowledged(h.ipc, "nonce-1")).toEqual({ nonce: "nonce-1", error: null });
    expect(log).toEqual([
      "load_update_recovery",
      "settle",
      "panel file-1",
      "get_last_transcription",
      "get_update_result_pending",
      "settle", // drainUpdateWork
      "settle",
      "get_update_result_pending",
      "load_update_recovery",
      "save_update_recovery",
      "complete_update_preparation",
    ]);
    expect(h.ipc.callsTo("save_update_recovery")).toEqual([{ payload: snapshot({ dictation: { text: DRAFT }, files: [fileEntry] }) }]);
    expect(h.controller.preparing).toBe(true); // only an abort or a failure clears it
  });

  test("a job without a registered panel fails the preparation as still opening", async () => {
    const h = harness({ files: { jobs: [completedJob("job-1", "/audio/a.wav")] } });
    h.controller.beginPreparation("nonce-1");

    expect(await acknowledged(h.ipc, "nonce-1")).toEqual({
      nonce: "nonce-1",
      error: "A transcription result is still opening. Retry the update.",
    });
    expect(h.ipc.callsTo("save_update_recovery")).toEqual([]);
    expect(h.controller.preparing).toBe(false);
  });

  test("a panel that registers while the view settles is found and asked", async () => {
    const prepareUpdateRecovery = vi.fn(() => Promise.resolve());
    const h = harness({
      files: { jobs: [completedJob("job-1", "/audio/a.wav")] },
      settle: async () => h.controller.setResultPanel("job-1", { prepareUpdateRecovery }),
    });
    h.controller.beginPreparation("nonce-1");

    expect(await acknowledged(h.ipc, "nonce-1")).toEqual({ nonce: "nonce-1", error: null });
    expect(prepareUpdateRecovery).toHaveBeenCalledTimes(1);
  });

  test("a panel that cannot finish its work fails the preparation with its message and nothing is saved", async () => {
    const h = harness({ files: { jobs: [completedJob("job-1", "/audio/a.wav")] } });
    h.controller.setResultPanel("job-1", {
      prepareUpdateRecovery: () => Promise.reject(new Error("The meeting review could not be saved.")),
    });
    h.controller.beginPreparation("nonce-1");

    expect(await acknowledged(h.ipc, "nonce-1")).toEqual({ nonce: "nonce-1", error: "The meeting review could not be saved." });
    expect(h.ipc.callsTo("save_update_recovery")).toEqual([]);
  });

  test("a save that fails is reported to the updater and ends the preparation", async () => {
    const h = harness({
      handlers: {
        save_update_recovery: () => {
          throw backendError("io", "Disk full.");
        },
      },
    });
    h.controller.beginPreparation("nonce-1");

    expect(await acknowledged(h.ipc, "nonce-1")).toEqual({ nonce: "nonce-1", error: "Disk full." });
    expect(h.controller.preparing).toBe(false);
  });

  test("a failure acknowledgement that itself fails is only logged", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const h = harness({
      handlers: {
        save_update_recovery: () => {
          throw backendError("io", "Disk full.");
        },
        complete_update_preparation: () => {
          throw backendError("updater", "The updater is gone.");
        },
      },
    });
    h.controller.beginPreparation("nonce-1");

    await vi.waitFor(() => expect(warn).toHaveBeenCalledWith("Could not acknowledge failed update preparation", expect.anything()));
    expect(h.ipc.callsTo("complete_update_preparation")).toEqual([{ nonce: "nonce-1", error: "Disk full." }]);
    expect(h.controller.preparing).toBe(false);
  });

  test.each([
    ["aborted", (controller: UpdateRecoveryController) => controller.abortPreparation()],
    ["disposed", (controller: UpdateRecoveryController) => controller.dispose()],
  ])("a preparation %s while a panel is busy stores and acknowledges nothing", async (_name, stop) => {
    const panelWork = deferred<void>();
    const prepareUpdateRecovery = vi.fn(() => panelWork.promise);
    const h = harness({ files: { jobs: [completedJob("job-1", "/audio/a.wav")] } });
    h.controller.setResultPanel("job-1", { prepareUpdateRecovery });
    h.controller.beginPreparation("nonce-1");
    await vi.waitFor(() => expect(prepareUpdateRecovery).toHaveBeenCalledTimes(1));

    stop(h.controller);
    panelWork.resolve();
    await settle();

    expect(h.ipc.calls).toEqual([]);
  });

  test("a preparation aborted while a backend read is held stores and acknowledges nothing", async () => {
    const heldRead = deferred<string | null>();
    const h = harness({ handlers: { get_last_transcription: () => heldRead.promise } });
    h.controller.beginPreparation("nonce-1");
    await vi.waitFor(() => expect(h.ipc.callsTo("get_last_transcription")).toHaveLength(1));

    h.controller.abortPreparation();
    expect(h.controller.preparing).toBe(false);
    heldRead.resolve(null);
    await settle();

    expect(commandsSince(h.ipc, 0)).toEqual(["get_last_transcription"]);
  });

  test("a newer preparation supersedes one that is still running", async () => {
    const firstPanelWork = deferred<void>();
    const h = harness({ files: { jobs: [completedJob("job-1", "/audio/a.wav")] } });
    let calls = 0;
    h.controller.setResultPanel("job-1", { prepareUpdateRecovery: () => (++calls === 1 ? firstPanelWork.promise : Promise.resolve()) });

    h.controller.beginPreparation("nonce-1");
    await vi.waitFor(() => expect(calls).toBe(1));
    h.controller.beginPreparation("nonce-2");
    await acknowledged(h.ipc, "nonce-2");
    firstPanelWork.resolve();
    await settle();

    expect(h.ipc.callsTo("complete_update_preparation")).toEqual([{ nonce: "nonce-2", error: null }]);
    expect(h.ipc.callsTo("save_update_recovery")).toHaveLength(1);
  });

  test.each([
    ["holds a pending native result the editor shows", { native: "Hej världen" }, "Hej världen"],
    ["holds an edited unsaved result", { dictation: { text: "Eget", recoveryPending: true, edited: true } }, "Eget"],
    ["leaves out a result that was already delivered", { dictation: { text: "Levererad", recoveryPending: false, edited: true } }, null],
    ["leaves out a result that is neither pending, recovered nor edited", { dictation: { text: "Test", recoveryPending: true } }, null],
    ["leaves out a blank edited result", { dictation: { text: "   ", recoveryPending: true, edited: true } }, null],
  ])("the snapshot %s", async (_name, setup: { native?: string; dictation?: Partial<DictationEditorPort> }, expected) => {
    const h = harness({ dictation: setup.dictation });
    if (setup.native) {
      const native = setup.native;
      h.ipc.handle("get_last_transcription", () => native);
      h.ipc.handle("get_update_result_pending", ({ resultId }) => resultId === "live-dictation");
      h.controller.showNativeResult(native);
    }
    h.controller.beginPreparation("nonce-1");

    expect(await acknowledged(h.ipc, "nonce-1")).toEqual({ nonce: "nonce-1", error: null });
    expect(h.ipc.callsTo("save_update_recovery")).toEqual([
      { payload: snapshot({ dictation: expected === null ? null : { text: expected } }) },
    ]);
  });

  test("a pending native result that differs from the editor refuses the update", async () => {
    const h = harness({
      dictation: { text: "Annan text", recoveryPending: true, edited: true },
      handlers: { get_last_transcription: () => "Native text", get_update_result_pending: () => true },
    });
    h.controller.nativeResultSettled("Native text"); // the editor is not empty, so it keeps its text
    h.controller.beginPreparation("nonce-1");

    expect(await acknowledged(h.ipc, "nonce-1")).toEqual({
      nonce: "nonce-1",
      error: "A different unsaved dictation is already open. Copy or save both results before retrying the update.",
    });
    expect(h.ipc.callsTo("save_update_recovery")).toEqual([]);
  });

  test("a stored dictation this session does not own refuses the update", async () => {
    const h = harness({
      initial: stored({ dictation: { text: "Gammalt utkast" } }),
      dictation: { text: "Ny text", recoveryPending: true, edited: true },
    });
    h.controller.beginPreparation("nonce-1"); // without restore(): the stored draft was never opened here

    expect(await acknowledged(h.ipc, "nonce-1")).toEqual({
      nonce: "nonce-1",
      error: "A different unsaved dictation is already in update recovery. Copy or save both results before retrying the update.",
    });
    expect(h.ipc.callsTo("save_update_recovery")).toEqual([]);
    expect(h.file.current).toEqual(stored({ dictation: { text: "Gammalt utkast" } }));
  });
});

describe("delivery (captureDelivery and markDelivered)", () => {
  test("delivering a native result acknowledges it and clears the editor's pending flags", async () => {
    const h = harness({
      handlers: { get_last_transcription: () => "Hej världen", get_update_result_pending: () => true },
    });
    h.controller.showNativeResult("Hej världen");

    const shownNext = await h.controller.markDelivered(h.controller.captureDelivery("Hej världen"));

    expect(shownNext).toBe(false);
    expect(h.ipc.callsTo("acknowledge_update_result")).toEqual([{ expectedText: "Hej världen" }]);
    expect(h.dictation).toMatchObject({ text: "Hej världen", recoveryPending: false, edited: false });
  });

  test("delivering the only restored draft clears the file and the drafts notice", async () => {
    const h = await restoredHarness({ initial: stored({ dictation: { text: DRAFT } }) });
    const from = h.ipc.calls.length;

    expect(await h.controller.markDelivered(h.controller.captureDelivery(DRAFT))).toBe(false);

    expect(commandsSince(h.ipc, from)).toEqual(["get_update_result_pending", "load_update_recovery", "clear_update_recovery"]);
    expect(h.controller.draftsNotice).toBe(false);
    expect(h.dictation.recoveryPending).toBe(false);
  });

  test("delivering a restored draft removes exactly that draft while another restored draft stays open", async () => {
    const h = await restoredHarness({ initial: stored({ dictation: { text: DRAFT }, files: [fileEntry] }) });

    await h.controller.markDelivered(h.controller.captureDelivery(DRAFT));

    expect(h.ipc.callsTo("save_update_recovery")).toEqual([{ payload: stored({ files: [fileEntry] }) }]);
    expect(h.controller.draftsNotice).toBe(true);
  });

  test("a delivery whose cleanup cannot read the file still succeeds and warns that the draft was retained", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const h = await restoredHarness({ initial: stored({ dictation: { text: DRAFT } }) });
    h.ipc.handle("load_update_recovery", () => {
      throw backendError("io", "Read failed.");
    });

    await expect(h.controller.markDelivered(h.controller.captureDelivery(DRAFT))).resolves.toBe(false);

    expect(h.controller.readError).toMatch(/^Update recovery draft was retained after delivery\. Read failed\./);
    expect(warn).toHaveBeenCalledWith("Could not clear delivered update recovery draft", expect.anything());
  });

  test("a restored draft delivered while a different native result is pending shows that result next", async () => {
    const h = await restoredHarness({ initial: stored({ dictation: { text: DRAFT } }) });
    h.ipc.handle("get_update_result_pending", () => true);
    h.ipc.handle("get_last_transcription", () => "Senaste diktering");

    expect(await h.controller.markDelivered(h.controller.captureDelivery(DRAFT))).toBe(true);

    expect(h.dictation).toMatchObject({ text: "Senaste diktering", recoveryPending: true });
    expect(h.ipc.callsTo("acknowledge_update_result")).toEqual([]);
  });

  test("a newer result that arrives during the delivery keeps its pending flag", async () => {
    const h = await restoredHarness({ initial: stored({ dictation: { text: DRAFT } }) });
    const heldPending = deferred<boolean>();
    h.ipc.handle("get_update_result_pending", () => heldPending.promise);

    const delivery = h.controller.markDelivered(h.controller.captureDelivery(DRAFT));
    await vi.waitFor(() => expect(h.ipc.callsTo("get_update_result_pending")).toHaveLength(1));
    h.controller.showNativeResult("Nyare diktering");
    heldPending.resolve(false);

    expect(await delivery).toBe(false);
    expect(h.dictation).toMatchObject({ text: "Nyare diktering", recoveryPending: true });
  });
});

describe("discard", () => {
  test("discarding closes the restored drafts and keeps the user's own jobs", async () => {
    const own = completedJob("file-2", otherFileEntry.path);
    const h = await restoredHarness({
      initial: stored({ dictation: { text: DRAFT }, files: [fileEntry], meetings: [meetingEntry] }),
      files: { jobs: [own], selectedId: null },
    });
    h.controller.onFileRecoveryChange(otherFileEntry); // the panel of the user's own job reports its entry
    h.files.selectedId = "meeting-1";
    const from = h.ipc.calls.length;

    await h.controller.discardDrafts();

    expect(commandsSince(h.ipc, from)).toEqual(["clear_update_recovery"]);
    expect(h.files.jobs).toEqual([own]);
    expect(h.files.savedReviewIds).toEqual([]);
    expect(h.files.selectedId).toBe("file-2");
    expect(h.controller.fileEntries).toEqual([otherFileEntry]);
    expect(h.controller.meetingEntries).toEqual([]);
    expect(h.dictation).toMatchObject({ text: "", recoveryPending: false, edited: false });
    expect(h.controller.draftsNotice).toBe(false);
  });

  test("discarding keeps an editor that no longer shows the restored text", async () => {
    const h = await restoredHarness({ initial: stored({ dictation: { text: DRAFT }, files: [fileEntry] }) });
    h.dictation.text = "Ändrad text";
    h.dictation.edited = true;

    await h.controller.discardDrafts();

    expect(h.dictation).toMatchObject({ text: "Ändrad text", recoveryPending: true, edited: true });
    expect(h.files.jobs).toEqual([]);
    expect(h.files.selectedId).toBeNull();
    expect(h.controller.draftsNotice).toBe(false);
  });

  test("a discard that fails reports why and closes nothing", async () => {
    const h = await restoredHarness({
      initial: stored({ dictation: { text: DRAFT }, files: [fileEntry] }),
      handlers: {
        clear_update_recovery: () => {
          throw backendError("io", "Permission denied");
        },
      },
    });

    await h.controller.discardDrafts();

    expect(h.discardFailures).toEqual(["Could not discard recovered drafts: Permission denied"]);
    expect(h.files.jobs).toEqual([completedJob("file-1", fileEntry.path)]);
    expect(h.controller.fileEntries).toEqual([fileEntry]);
    expect(h.dictation).toMatchObject({ text: DRAFT, recoveryPending: true });
    expect(h.controller.draftsNotice).toBe(true);
  });
});
