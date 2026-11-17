// Record.svelte (the recorder window) against the IPC mock (SAGA-564): what a
// user sees and can do, driven through roles and text. The pure decisions are
// tested in record-state.test.ts.
import { mockConvertFileSrc } from "@tauri-apps/api/mocks";
import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";
import Record from "./Record.svelte";
import { backendError } from "../test/ipc";
import {
  defaultFolder,
  finishedRecording,
  recorderBackend,
  recoveryNote,
  sessionInfo,
  summary,
  type RecorderBackend,
} from "../test/record";

const dialog = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => dialog);

beforeEach(() => {
  dialog.open.mockReset();
});

async function renderRecord(backend: RecorderBackend, props: { playbackFallback?: boolean } = {}) {
  render(Record, props);
  await waitFor(() => expect(backend.listenerCount("recording-session-changed")).toBe(1));
  await waitFor(() => expect(backend.callsTo("list_recordings").length).toBeGreaterThan(0));
  return userEvent.setup();
}

const startButton = () => screen.getByRole("button", { name: "Start recording" });
const stopButton = () => screen.getByRole("button", { name: "Stop recording" });

async function startedRecording(backend: RecorderBackend, user: ReturnType<typeof userEvent.setup>) {
  await waitFor(() => expect(startButton()).toBeEnabled());
  await user.click(startButton());
  await screen.findByRole("button", { name: "Stop recording" });
  return backend;
}

describe("first use", () => {
  test("recording cannot start until the folder is confirmed and the consent reminder acknowledged", async () => {
    const backend = recorderBackend({}, { recording_folder_confirmed: false, recording_consent_acknowledged: false });
    const user = await renderRecord(backend);

    expect(await screen.findByRole("heading", { name: "Where should recordings go?" })).toBeInTheDocument();
    expect(screen.getByText(defaultFolder.path, { selector: "section.step code" })).toBeInTheDocument();
    expect(startButton()).toBeDisabled();
    expect(screen.queryByRole("heading", { name: "Before you record" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Use this folder" }));
    expect(await screen.findByRole("heading", { name: "Before you record" })).toBeInTheDocument();
    expect(backend.callsTo("confirm_recording_folder")).toHaveLength(1);
    expect(startButton()).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "I understand" }));
    await waitFor(() => expect(startButton()).toBeEnabled());
    expect(backend.callsTo("acknowledge_recording_consent")).toHaveLength(1);
    expect(screen.queryByRole("heading", { name: "Before you record" })).not.toBeInTheDocument();
    expect(backend.callsTo("start_recording_session")).toEqual([]);
  });

  test("a returning user sees no first-use steps", async () => {
    const backend = recorderBackend();
    await renderRecord(backend);
    await waitFor(() => expect(startButton()).toBeEnabled());
    expect(screen.queryByRole("heading", { name: "Where should recordings go?" })).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Before you record" })).not.toBeInTheDocument();
  });

  test("a failed confirmation keeps the step and says why", async () => {
    const backend = recorderBackend(
      {
        confirm_recording_folder: () => {
          throw backendError("recording_io", "Could not save the setting.", true);
        },
      },
      { recording_folder_confirmed: false },
    );
    const user = await renderRecord(backend);
    await user.click(await screen.findByRole("button", { name: "Use this folder" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save the setting.");
    expect(screen.getByRole("heading", { name: "Where should recordings go?" })).toBeInTheDocument();
  });
});

describe("recording", () => {
  test("start and stop invoke the commands and show the result", async () => {
    const backend = recorderBackend();
    const user = await renderRecord(backend);
    await startedRecording(backend, user);

    expect(backend.callsTo("start_recording_session")).toEqual([{ title: null }]);
    expect(screen.getByRole("status", { name: "Recording state" })).toHaveTextContent("Recording");
    expect(screen.queryByRole("button", { name: "Start recording" })).not.toBeInTheDocument();

    await user.click(stopButton());
    const saved = await screen.findByRole("region", { name: "Finished recording" });
    expect(backend.callsTo("stop_recording_session")).toHaveLength(1);
    expect(saved).toHaveTextContent("Saved: Interview");
    expect(saved).toHaveTextContent("1:02 · 5.1 MB");
    expect(within(saved).getByRole("button", { name: /Reveal in Finder: Interview/ })).toBeInTheDocument();
    expect(within(saved).getByRole("button", { name: "Transcribe Interview" })).toBeInTheDocument();
    expect(within(saved).getByRole("button", { name: /Move to Trash: Interview/ })).toBeInTheDocument();
    // The list was read again after the stop.
    expect(backend.callsTo("list_recordings").length).toBeGreaterThanOrEqual(2);
    expect(startButton()).toBeEnabled();
  });

  test("elapsed time, level and notices follow the session events", async () => {
    const backend = recorderBackend();
    const user = await renderRecord(backend);
    await startedRecording(backend, user);

    backend.emit("recording-session-changed", sessionInfo({ elapsed_ms: 65_400, level: 0.0316 }));
    await waitFor(() => expect(screen.getByRole("timer", { name: "Elapsed time" })).toHaveTextContent("0:01:05"));
    expect(screen.getByRole("meter", { name: "Input level" })).toHaveAttribute("aria-valuenow", "50");
    expect(screen.getByRole("meter", { name: "Input level" })).toHaveAttribute("aria-valuetext", "-30 dB");

    backend.emit(
      "recording-session-changed",
      sessionInfo({ elapsed_ms: 3_661_000, level: 1, clipped: true, gap_count: 1, gap_ms: 400, warnings: ["low_disk"] }),
    );
    await waitFor(() => expect(screen.getByRole("timer", { name: "Elapsed time" })).toHaveTextContent("1:01:01"));
    expect(screen.getByRole("meter", { name: "Input level" })).toHaveAttribute("aria-valuenow", "100");
    const notices = screen.getByRole("list", { name: "Recording notices" });
    expect(notices).toHaveTextContent("The input is clipping");
    expect(notices).toHaveTextContent("1 gap in the audio (0.4 s of silence).");
    expect(notices).toHaveTextContent("Low disk space");
  });

  test("a session started elsewhere is followed", async () => {
    const backend = recorderBackend();
    await renderRecord(backend);
    backend.emit("recording-session-changed", sessionInfo({ elapsed_ms: 2000 }));
    expect(await screen.findByRole("button", { name: "Stop recording" })).toBeEnabled();
    expect(screen.getByRole("timer", { name: "Elapsed time" })).toHaveTextContent("0:00:02");
  });

  test("a session that is already running when the window opens is shown", async () => {
    const backend = recorderBackend({ get_recording_session: () => sessionInfo({ elapsed_ms: 125_000 }) });
    await renderRecord(backend);
    expect(await screen.findByRole("button", { name: "Stop recording" })).toBeInTheDocument();
    expect(screen.getByRole("timer", { name: "Elapsed time" })).toHaveTextContent("0:02:05");
  });

  test("the quality is chosen with its size per hour and locked while recording", async () => {
    const backend = recorderBackend();
    const user = await renderRecord(backend);
    const lossless = screen.getByRole("radio", { name: "Lossless" });
    const compact = screen.getByRole("radio", { name: "Compact" });
    expect(lossless).toBeChecked();
    expect(lossless).toHaveAccessibleDescription("about 295 MB per hour");
    expect(compact).toHaveAccessibleDescription("about 46.0 MB per hour");

    await user.click(compact);
    await waitFor(() => expect(backend.callsTo("set_recording_quality")).toEqual([{ quality: "compact" }]));
    expect(compact).toBeChecked();

    await startedRecording(backend, user);
    expect(compact).toBeDisabled();
    expect(lossless).toBeDisabled();
    expect(screen.getByRole("button", { name: "Change…" })).toBeDisabled();
  });

  test("the saved quality is the one selected", async () => {
    const backend = recorderBackend({}, { recording_quality: "compact" });
    await renderRecord(backend);
    await waitFor(() => expect(screen.getByRole("radio", { name: "Compact" })).toBeChecked());
  });
});

describe("failures", () => {
  test.each([
    ["recording_permission_denied", "Sagascript is not allowed to use the microphone."],
    ["recording_source_unavailable", "No microphone is available."],
    ["recording_disk_full", "The disk is full."],
    ["recording_device_changed", "The microphone changed or was unplugged."],
  ])("a refused start with %s shows its text and can be retried", async (code, text) => {
    let attempts = 0;
    const backend = recorderBackend({
      start_recording_session: () => {
        attempts++;
        if (attempts === 1) throw backendError(code, "backend words", true);
        return sessionInfo();
      },
    });
    const user = await renderRecord(backend);
    await waitFor(() => expect(startButton()).toBeEnabled());
    await user.click(startButton());

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(text);
    expect(alert).not.toHaveTextContent("backend words");

    await user.click(within(alert).getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("button", { name: "Stop recording" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(backend.callsTo("start_recording_session")).toHaveLength(2);
  });

  test("an interrupted recording offers recovery instead of a retry", async () => {
    const backend = recorderBackend({
      start_recording_session: () => {
        throw backendError("recording_recovery_needed", "x");
      },
      recover_recordings: () => [recoveryNote()],
    });
    const user = await renderRecord(backend);
    await waitFor(() => expect(startButton()).toBeEnabled());
    await user.click(startButton());
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("has to be recovered");
    expect(within(alert).queryByRole("button", { name: "Try again" })).not.toBeInTheDocument();
    await user.click(within(alert).getByRole("button", { name: "Recover" }));
    expect(await screen.findByRole("region", { name: "Recovered recordings" })).toBeInTheDocument();
    expect(screen.queryByText(/has to be recovered/)).not.toBeInTheDocument();
  });

  test("an unknown code shows the backend's message", async () => {
    const backend = recorderBackend({
      start_recording_session: () => {
        throw backendError("recording_encoder", "The encoder stopped.", false);
      },
    });
    const user = await renderRecord(backend);
    await waitFor(() => expect(startButton()).toBeEnabled());
    await user.click(startButton());
    expect(await screen.findByRole("alert")).toHaveTextContent("The encoder stopped.");
  });

  test("a session that fails while recording shows its error code", async () => {
    const backend = recorderBackend();
    const user = await renderRecord(backend);
    await startedRecording(backend, user);
    backend.emit("recording-session-changed", sessionInfo({ state: "failed", error_code: "recording_disk_full" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The disk is full.");
    expect(startButton()).toBeEnabled();
  });

  test("a recording that ended early says why", async () => {
    const backend = recorderBackend({
      stop_recording_session: () => ({ ...finishedRecording, ended_early: "device_changed" }),
    });
    const user = await renderRecord(backend);
    await startedRecording(backend, user);
    await user.click(stopButton());
    const saved = await screen.findByRole("region", { name: "Finished recording" });
    expect(within(saved).getByRole("alert")).toHaveTextContent("ended early because the microphone changed");
  });

  test("a failed stop is reported", async () => {
    const backend = recorderBackend({
      stop_recording_session: () => {
        throw backendError("recording_io", "Could not finish the file.", true);
      },
    });
    const user = await renderRecord(backend);
    await startedRecording(backend, user);
    await user.click(stopButton());
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not finish the file.");
  });
});

describe("folder", () => {
  test("the folder, its free space and its warnings are shown", async () => {
    const backend = recorderBackend({
      get_recording_folder_info: () => ({ ...defaultFolder, warnings: ["fat32", "cloud_synced"] }),
    });
    await renderRecord(backend);
    const folder = await screen.findByRole("region", { name: "Recording folder" });
    expect(await within(folder).findByText(defaultFolder.path)).toBeInTheDocument();
    expect(folder).toHaveTextContent("120 GB free");
    const warnings = within(folder).getByRole("list", { name: "Folder warnings" });
    expect(warnings).toHaveTextContent("FAT32");
    expect(warnings).toHaveTextContent("synced to the cloud");
  });

  test("changing the folder sets and confirms the chosen directory", async () => {
    dialog.open.mockResolvedValue("/Volumes/Work/Talks");
    let folder = defaultFolder;
    const backend = recorderBackend({
      get_recording_folder_info: () => folder,
      set_recording_directory: () => {
        folder = { ...defaultFolder, path: "/Volumes/Work/Talks", is_default: false };
        return null;
      },
    });
    const user = await renderRecord(backend);
    await user.click(screen.getByRole("button", { name: "Change…" }));
    expect(await screen.findByText("/Volumes/Work/Talks")).toBeInTheDocument();
    expect(dialog.open).toHaveBeenCalledWith(expect.objectContaining({ directory: true, multiple: false }));
    expect(backend.callsTo("set_recording_directory")).toEqual([{ path: "/Volumes/Work/Talks" }]);
    expect(backend.callsTo("confirm_recording_folder")).toHaveLength(1);
  });

  test("cancelling the folder dialog changes nothing", async () => {
    dialog.open.mockResolvedValue(null);
    const backend = recorderBackend();
    const user = await renderRecord(backend);
    await user.click(screen.getByRole("button", { name: "Change…" }));
    await waitFor(() => expect(dialog.open).toHaveBeenCalled());
    expect(backend.callsTo("set_recording_directory")).toEqual([]);
  });
});

describe("recordings", () => {
  const talk = summary({ folder_name: "2026-11-05 15.30 Talk", title: "Talk", recording_id: "talk-1", duration_seconds: 3_725 });
  const interrupted = summary({
    folder_name: "2026-11-04 09.00 Lost",
    title: "Lost",
    recording_id: "lost-1",
    state: "recording",
    needs_recovery: true,
    playable: false,
  });

  test("the list shows recent recordings with their actions", async () => {
    const backend = recorderBackend({ list_recordings: () => [talk, interrupted] });
    await renderRecord(backend);
    const items = await screen.findAllByRole("listitem");
    const first = items.find((item) => item.textContent?.includes("Talk"))!;
    expect(first).toHaveTextContent("1:02:05");
    expect(within(first).getByRole("button", { name: /Reveal in Finder: Talk/ })).toBeInTheDocument();
    const lost = items.find((item) => item.textContent?.includes("Lost"))!;
    expect(lost).toHaveTextContent("Interrupted");
    expect(within(lost).getByRole("button", { name: "Recover Lost" })).toBeInTheDocument();
    expect(within(lost).queryByRole("button", { name: /Transcribe/ })).not.toBeInTheDocument();
  });

  test("an empty list says so", async () => {
    await renderRecord(recorderBackend());
    expect(await screen.findByText("No recordings yet.")).toBeInTheDocument();
  });

  test("reveal, transcribe and move to Trash act on the recording", async () => {
    let listed = [talk];
    const backend = recorderBackend({ list_recordings: () => listed });
    const user = await renderRecord(backend);

    await user.click(await screen.findByRole("button", { name: /Reveal in Finder: Talk/ }));
    await waitFor(() => expect(backend.callsTo("reveal_recording")).toEqual([{ folderName: talk.folder_name }]));

    await user.click(screen.getByRole("button", { name: "Transcribe Talk" }));
    const transcript = await screen.findByRole("region", { name: "Transcript: Talk" });
    expect(transcript).toHaveTextContent("Hello from the recording.");
    expect(backend.callsTo("transcribe_recording")).toEqual([{ folderName: talk.folder_name }]);

    listed = [];
    await user.click(screen.getByRole("button", { name: /Move to Trash: Talk/ }));
    await waitFor(() => expect(backend.callsTo("delete_recording")).toEqual([{ folderName: talk.folder_name }]));
    expect(await screen.findByText("No recordings yet.")).toBeInTheDocument();
    expect(screen.getByText("Moved “Talk” to the Trash.")).toBeInTheDocument();
  });

  test("the finished recording can be moved to the Trash from its card", async () => {
    const backend = recorderBackend();
    const user = await renderRecord(backend);
    await startedRecording(backend, user);
    await user.click(stopButton());
    const saved = await screen.findByRole("region", { name: "Finished recording" });
    await user.click(within(saved).getByRole("button", { name: /Move to Trash/ }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Finished recording" })).not.toBeInTheDocument());
    expect(backend.callsTo("delete_recording")).toEqual([{ folderName: finishedRecording.summary.folder_name }]);
  });

  test("a failing action shows its error", async () => {
    const backend = recorderBackend({
      list_recordings: () => [talk],
      transcribe_recording: () => {
        throw backendError("recording_corrupt_track", "The track is damaged.");
      },
    });
    const user = await renderRecord(backend);
    await user.click(await screen.findByRole("button", { name: "Transcribe Talk" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The track is damaged.");
  });

  test("Windows names Explorer and the Recycle Bin", async () => {
    const backend = recorderBackend({ list_recordings: () => [talk], get_platform: () => "windows" });
    await renderRecord(backend);
    expect(await screen.findByRole("button", { name: /Reveal in Explorer: Talk/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Move to Recycle Bin: Talk/ })).toBeInTheDocument();
  });
});

describe("playback", () => {
  const playable = summary({ folder_name: "a", title: "Playable", playable: true });
  const notPlayable = summary({ folder_name: "b", title: "Broken", playable: false, needs_recovery: true });

  test("the fallback replaces the player with a pointer to Reveal", async () => {
    const backend = recorderBackend({ list_recordings: () => [playable] });
    await renderRecord(backend, { playbackFallback: true });
    expect(await screen.findByText("Playback not verified, use Reveal.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Play/ })).not.toBeInTheDocument();
  });

  test("without the fallback a playable recording has a player", async () => {
    const backend = recorderBackend({ list_recordings: () => [playable] });
    const user = await renderRecord(backend, { playbackFallback: false });
    mockConvertFileSrc("macos");
    await user.click(await screen.findByRole("button", { name: "Play: Playable" }));
    const player = await screen.findByLabelText("Recording audio: Playable");
    expect(player.tagName).toBe("AUDIO");
    expect(player).toHaveAttribute("src", expect.stringContaining("recording-audio"));
    await user.click(screen.getByRole("button", { name: "Hide player: Playable" }));
    expect(screen.queryByLabelText("Recording audio: Playable")).not.toBeInTheDocument();
  });

  test("a recording that is not playable has no player and no fallback note", async () => {
    const backend = recorderBackend({ list_recordings: () => [notPlayable] });
    await renderRecord(backend, { playbackFallback: false });
    await screen.findByText("Broken");
    expect(screen.queryByRole("button", { name: /^Play/ })).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/Recording audio/)).not.toBeInTheDocument();
  });

  test("a recording that is not playable shows no fallback note either", async () => {
    const backend = recorderBackend({ list_recordings: () => [notPlayable] });
    await renderRecord(backend, { playbackFallback: true });
    await screen.findByText("Broken");
    expect(screen.queryByText("Playback not verified, use Reveal.")).not.toBeInTheDocument();
  });
});

describe("recovery", () => {
  test("a recovery at app start is reported and can be dismissed", async () => {
    const backend = recorderBackend();
    const user = await renderRecord(backend);
    expect(screen.queryByRole("region", { name: "Recovered recordings" })).not.toBeInTheDocument();

    backend.emit("recordings-recovered", [recoveryNote(), recoveryNote({ folder_name: "x", title: "Done", outcome: "already_complete" })]);
    const note = await screen.findByRole("region", { name: "Recovered recordings" });
    expect(note).toHaveTextContent("Recovered 1 recording");
    expect(note).toHaveTextContent("Lecture: Kept 10 minutes of audio; the last second was lost.");
    expect(note).not.toHaveTextContent("Done");

    await user.click(within(note).getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByRole("region", { name: "Recovered recordings" })).not.toBeInTheDocument();
  });

  test("notes kept from app start are shown when the window opens", async () => {
    const backend = recorderBackend({ recover_recordings: () => [recoveryNote(), recoveryNote({ folder_name: "y", title: "Memo" })] });
    await renderRecord(backend);
    const note = await screen.findByRole("region", { name: "Recovered recordings" });
    expect(note).toHaveTextContent("Recovered 2 recordings");
  });
});

describe("lifecycle", () => {
  test("closing the window unlistens", async () => {
    const backend = recorderBackend();
    const view = render(Record);
    await waitFor(() => expect(backend.listenerCount("recording-session-changed")).toBe(1));
    expect(backend.listenerCount("recordings-recovered")).toBe(1);
    view.unmount();
    await waitFor(() => expect(backend.ipc.listeners()).toEqual([]));
  });
});
