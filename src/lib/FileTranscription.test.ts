// FileTranscription.svelte inside Settings (#362): a plain run listens for its
// own progress events from before the native work starts until it ends.
import { emit } from "@tauri-apps/api/event";
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { deferred } from "../test/deferred";
import { backendError } from "../test/ipc";
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
        throw backendError("file_decode_error", "Could not decode the file");
      },
      set_update_result_pending: () => undefined,
    }),
  );
  await userEvent.setup().click(screen.getByRole("button", { name: "Transcribe" }));

  await emit("tauri://drag-drop", { paths: ["/audio/broken.wav"], position: { x: 0, y: 0 } });

  expect(await screen.findByText("Could not decode the file")).toBeInTheDocument();
  expect(ipc.listenerCount("plain-transcription-progress")).toBe(0);
});

// A run the backend ended as `cancelled` (the engine was stopped, not only the
// Stop button) is a cancelled run, not a failed one (ADR 0006, #360 part 2b).
// The existing cancelled wording is used: "Transcription was cancelled." in the
// panel, "Cancelled" on the stage, `cancelled` on the file's tab and in the
// queue summary.
async function dropAndReject(failure: unknown, path: string) {
  const rendered = await renderSettings(
    settingsBackend({
      transcribe_file: () => {
        throw failure;
      },
      set_update_result_pending: () => undefined,
    }),
  );
  await userEvent.setup().click(screen.getByRole("button", { name: "Transcribe" }));
  await emit("tauri://drag-drop", { paths: [path], position: { x: 0, y: 0 } });
  await waitFor(() => expect(rendered.ipc.callsTo("transcribe_file")).toHaveLength(1));
  return rendered;
}

const queueSummary = () => screen.getByRole("status", { name: "File transcription status" }).textContent?.replace(/\s+/g, " ").trim();

test("a backend cancellation shows as cancelled, not as a failure", async () => {
  const { ipc } = await dropAndReject(backendError("cancelled", "Transcription cancelled."), "/audio/stopped.wav");

  expect(await screen.findByText("Transcription was cancelled.")).toBeInTheDocument();
  expect(screen.queryByText("Transcription cancelled.")).toBeNull();
  expect(screen.queryByText("Transcription failed")).toBeNull();
  await waitFor(() => expect(queueSummary()).toMatch(/^0 completed · 0 failed · 1 cancelled /));
  expect(screen.getByRole("tab", { name: /stopped\.wav/ })).toHaveTextContent("cancelled");
  expect(screen.getByRole("list", { name: "Transcription steps" })).toHaveTextContent("Cancelled");
  expect(screen.getByRole("list", { name: "Transcription steps" })).not.toHaveTextContent("Failed");
  expect(ipc.listenerCount("plain-transcription-progress")).toBe(0);
});

test("another backend error is still a failure, with its own message", async () => {
  await dropAndReject(backendError("engine_busy", "The engine is busy.", true), "/audio/busy.wav");

  expect(await screen.findByText("The engine is busy.")).toBeInTheDocument();
  await waitFor(() => expect(queueSummary()).toMatch(/^0 completed · 1 failed · 0 cancelled /));
  expect(screen.getByRole("tab", { name: /busy\.wav/ })).toHaveTextContent("failed");
  expect(screen.queryByText("Transcription was cancelled.")).toBeNull();
});
