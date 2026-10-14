// FileTranscription.svelte inside Settings (#362): a plain run listens for its
// own progress events from before the native work starts until it ends.
import { emit } from "@tauri-apps/api/event";
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { deferred } from "../test/deferred";
import { renderSettings, settingsBackend } from "../test/settings";

test("a plain transcription listens for progress while it runs, and stops when it ends", async () => {
  const transcription = deferred<string>();
  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: () => transcription.promise,
      set_update_result_pending: () => undefined,
    }),
  );
  await userEvent.setup().click(screen.getByRole("button", { name: "Transcribe" }));
  expect(ipc.listenerCount("plain-transcription-progress")).toBe(0);

  // The window receives a dropped file as the webview's drag and drop event.
  await emit("tauri://drag-drop", { paths: ["/audio/interview.wav"], position: { x: 0, y: 0 } });

  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(1));
  const { runId } = ipc.callsTo("transcribe_file")[0];
  // Registered before the native call, so the first event is not missed.
  expect(ipc.listenerCount("plain-transcription-progress")).toBe(1);
  expect(runId).toEqual(expect.any(String));
  expect(ipc.emit("plain-transcription-progress", { runId: runId!, phase: "resampling", percent: 40 })).toBe(1);

  transcription.resolve("Intervjun i text");

  await waitFor(() => expect(ipc.listenerCount("plain-transcription-progress")).toBe(0));
  expect(await screen.findByDisplayValue("Intervjun i text")).toBeInTheDocument();
});

test("a run that fails still releases its progress listener", async () => {
  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: () => {
        throw "Could not decode the file";
      },
      set_update_result_pending: () => undefined,
    }),
  );
  await userEvent.setup().click(screen.getByRole("button", { name: "Transcribe" }));

  await emit("tauri://drag-drop", { paths: ["/audio/broken.wav"], position: { x: 0, y: 0 } });

  expect(await screen.findByText("Could not decode the file")).toBeInTheDocument();
  expect(ipc.listenerCount("plain-transcription-progress")).toBe(0);
});
