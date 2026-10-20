// FileTranscription.svelte inside Settings (#362): a plain run listens for its
// own progress events from before the native work starts until it ends.
import { emit } from "@tauri-apps/api/event";
import { screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { deferred, settle } from "../test/deferred";
import { backendError } from "../test/ipc";
import { pianissimo, renderSettings, settingsBackend, settingsValue, swedishProfile } from "../test/settings";
import commandErrors from "../../contracts/transport/command_errors.json";

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

// The way out of a failed run is decided by the error's code (ADR 0006): "Try
// again" runs the file again as a new job, "Download the model again" repairs
// the engine's model. Neither looks at the message.
const tryAgain = () => screen.queryByRole("button", { name: "Try again" });
const downloadAgain = () => screen.queryByRole("button", { name: "Download the model again" });

test("Try again runs the same file again as a new job and keeps the failed one", async () => {
  let calls = 0;
  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: () => {
        calls += 1;
        if (calls === 1) throw commandErrors.model_busy;
        return "Intervjun i text";
      },
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Transcribe" }));
  await emit("tauri://drag-drop", { paths: ["/audio/interview.wav"], position: { x: 0, y: 0 } });
  expect(await screen.findByText(commandErrors.model_busy.message)).toBeInTheDocument();

  await user.click(await screen.findByRole("button", { name: "Try again" }));

  expect(await screen.findByDisplayValue("Intervjun i text")).toBeInTheDocument();
  const runs = ipc.callsTo("transcribe_file");
  expect(runs).toHaveLength(2);
  expect(runs[1].filePath).toBe(runs[0].filePath);
  expect(runs[1].runId).not.toBe(runs[0].runId);
  await waitFor(() => expect(queueSummary()).toMatch(/^1 completed · 1 failed · 0 cancelled /));
  expect(screen.getAllByRole("tab", { name: /interview\.wav/ })).toHaveLength(2);
  expect(tryAgain()).toBeNull();
});

test("Try again is offered by the retry hint, whatever the message says", async () => {
  await dropAndReject(backendError("engine_busy", "Pianissimo engine: queue full", true), "/audio/queued.wav");

  expect(await screen.findByRole("button", { name: "Try again" })).not.toHaveAttribute("aria-disabled", "true");
  expect(downloadAgain()).toBeNull();
});

test.each([
  ["transcription_timed_out", commandErrors.transcription_timed_out],
  ["file_not_found", commandErrors.file_not_found],
  ["cancelled", commandErrors.cancelled],
  ["invalid_input", backendError("invalid_input", "That did not work. Please try again.")],
])("neither button is offered for %s", async (_code, failure) => {
  await dropAndReject(failure, "/audio/final.wav");

  await waitFor(() => expect(queueSummary()).toMatch(/^0 completed · [01] failed · [01] cancelled /));
  expect(tryAgain()).toBeNull();
  expect(downloadAgain()).toBeNull();
});

test("a missing engine model offers the download, then Try again", async () => {
  const download = deferred<void>();
  let calls = 0;
  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: () => {
        calls += 1;
        if (calls === 1) throw commandErrors.engine_model_missing;
        return "Efter nedladdningen";
      },
      download_pianissimo_model: () => download.promise,
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Transcribe" }));
  await emit("tauri://drag-drop", { paths: ["/audio/swedish.wav"], position: { x: 0, y: 0 } });
  const downloadButton = await screen.findByRole("button", { name: "Download the model again" });
  expect(tryAgain()).toBeNull();

  await user.click(downloadButton);

  await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1));
  await waitFor(() => expect(screen.getByRole("button", { name: "Download the model again" })).toHaveAttribute("aria-disabled", "true"));
  download.resolve();
  const retry = await screen.findByRole("button", { name: "Try again" });
  expect(downloadAgain()).toBeNull();

  await user.click(retry);

  expect(await screen.findByDisplayValue("Efter nedladdningen")).toBeInTheDocument();
  expect(ipc.callsTo("transcribe_file")).toHaveLength(2);
});

test("a failed download shows its error and offers the download again", async () => {
  const { ipc } = await dropAndReject(commandErrors.engine_model_load_failed, "/audio/broken-model.wav");
  ipc.handle("download_pianissimo_model", () => {
    throw backendError("model_download_failed", "Model download failed: offline");
  });

  await userEvent.setup().click(await screen.findByRole("button", { name: "Download the model again" }));

  expect(await screen.findByText("Model download failed: offline")).toBeInTheDocument();
  expect(await screen.findByRole("button", { name: "Download the model again" })).not.toHaveAttribute("aria-disabled", "true");
  expect(tryAgain()).toBeNull();
});

// Focus must not fall to the page at a recovery transition (#360 review).
test("Try again moves focus to the tab of the new job", async () => {
  let calls = 0;
  await renderSettings(
    settingsBackend({
      transcribe_file: () => {
        calls += 1;
        if (calls === 1) throw commandErrors.model_busy;
        return new Promise<string>(() => undefined);
      },
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Transcribe" }));
  await emit("tauri://drag-drop", { paths: ["/audio/interview.wav"], position: { x: 0, y: 0 } });

  await user.click(await screen.findByRole("button", { name: "Try again" }));

  await waitFor(() => expect(screen.getAllByRole("tab", { name: /interview\.wav/ })).toHaveLength(2));
  const [, newest] = screen.getAllByRole("tab", { name: /interview\.wav/ });
  expect(newest).toHaveAttribute("aria-selected", "true");
  await waitFor(() => expect(newest).toHaveFocus());
});

test("a Try again the window cannot queue stays on screen and keeps the focus", async () => {
  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: () => {
        throw commandErrors.model_busy;
      },
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Transcribe" }));
  await emit("tauri://drag-drop", { paths: ["/audio/gone-profile.wav"], position: { x: 0, y: 0 } });
  const retry = await screen.findByRole("button", { name: "Try again" });
  // The profile the file ran with is removed meanwhile.
  ipc.handle("get_settings", () => settingsValue({ hotkey_profiles: [{ ...swedishProfile, id: "other", name: "Other" }] }));
  ipc.emit("state-changed", "settings_reloaded");
  await waitFor(() => expect(ipc.callsTo("get_settings").length).toBeGreaterThan(1));
  await settle();

  await user.click(retry);

  // The refusal is announced, and shown in the failed file's panel beside the
  // button that was pressed, once: not above the tab strip, out of sight.
  const refusal = await within(screen.getByRole("tabpanel")).findByText(/profile for this file is no longer available/);
  expect(refusal).toHaveAttribute("role", "alert");
  expect(screen.getAllByText(/profile for this file is no longer available/)).toHaveLength(1);
  expect(ipc.callsTo("transcribe_file")).toHaveLength(1);
  expect(screen.getByRole("button", { name: "Try again" })).toBe(retry);
  expect(retry).toHaveFocus();

  // The next Try again that can be queued takes the refusal away.
  ipc.handle("get_settings", () => settingsValue());
  ipc.emit("state-changed", "settings_reloaded");
  await settle();
  await user.click(retry);
  await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(2));
  expect(screen.queryByText(/profile for this file is no longer available/)).toBeNull();
});

test("a Re-run the window cannot queue is announced", async () => {
  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: () => "Text",
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Transcribe" }));
  await emit("tauri://drag-drop", { paths: ["/audio/rerun.wav"], position: { x: 0, y: 0 } });
  await screen.findByDisplayValue("Text");
  ipc.handle("get_settings", () => settingsValue({ hotkey_profiles: [{ ...swedishProfile, id: "other", name: "Other" }] }));
  ipc.emit("state-changed", "settings_reloaded");
  await settle();

  await user.click(screen.getByRole("button", { name: "Re-run" }));

  const refusal = await screen.findByText(/profile for this file is no longer available/);
  expect(refusal).toHaveAttribute("role", "alert");
});

test("a model download keeps focus on its button and hands it to the Try again that follows", async () => {
  const download = deferred<void>();
  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: () => {
        throw commandErrors.engine_model_missing;
      },
      download_pianissimo_model: () => download.promise,
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Transcribe" }));
  await emit("tauri://drag-drop", { paths: ["/audio/swedish.wav"], position: { x: 0, y: 0 } });
  const button = await screen.findByRole("button", { name: "Download the model again" });

  await user.click(button);

  await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1));
  expect(button).toHaveAttribute("aria-disabled", "true");
  expect(button).not.toBeDisabled();
  expect(button).toHaveFocus();
  await user.click(button);
  expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1);

  download.resolve();

  const retry = await screen.findByRole("button", { name: "Try again" });
  expect(retry).toHaveFocus();
});

test.each([
  ["pianissimo_unavailable", backendError("pianissimo_unavailable", "Pianissimo is not available on this system.")],
  ["invalid_input", backendError("invalid_input", "That did not work.")],
])("a download refused with %s shows the error and offers nothing, so it cannot loop", async (_code, refusal) => {
  const { ipc } = await dropAndReject(commandErrors.engine_model_missing, "/audio/refused.wav");
  ipc.handle("download_pianissimo_model", () => {
    throw refusal;
  });

  await userEvent.setup().click(await screen.findByRole("button", { name: "Download the model again" }));

  expect(await screen.findByText(refusal.message)).toBeInTheDocument();
  await settle();
  expect(downloadAgain()).toBeNull();
  expect(tryAgain()).toBeNull();
  expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1);
});

test("a failed run's error is an alert, announced with its button", async () => {
  await dropAndReject(commandErrors.model_busy, "/audio/announced.wav");

  const panel = screen.getByRole("tabpanel");
  const alert = await within(panel).findByRole("alert");
  expect(alert).toHaveTextContent(commandErrors.model_busy.message);
});

test("while the model list is refreshed after a download, no file's button can start another", async () => {
  const download = deferred<void>();
  const refresh = deferred<typeof pianissimo>();
  let holdRefresh = false;
  const { ipc } = await renderSettings(
    settingsBackend({
      transcribe_file: () => {
        throw commandErrors.engine_model_missing;
      },
      download_pianissimo_model: () => download.promise,
      get_profile_model_info: () => (holdRefresh ? refresh.promise : pianissimo),
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Transcribe" }));
  await emit("tauri://drag-drop", { paths: ["/audio/one.wav", "/audio/two.wav"], position: { x: 0, y: 0 } });
  await waitFor(() => expect(screen.getAllByRole("tab", { name: /failed/ })).toHaveLength(2));
  const buttons = () => screen.getAllByRole("button", { name: "Download the model again", hidden: true });
  await waitFor(() => expect(buttons()).toHaveLength(2));
  const [first, second] = buttons();
  await user.click(first);
  await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1));
  const before = ipc.callsTo("get_profile_model_info").length;
  holdRefresh = true;

  download.resolve();
  await waitFor(() => expect(ipc.callsTo("get_profile_model_info").length).toBeGreaterThan(before));

  expect(second).toHaveAttribute("aria-disabled", "true");
  refresh.resolve(pianissimo);
  await waitFor(() => expect(first).toHaveTextContent("Try again"));
  expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1);
});

test("Try again shows as busy while an update is being prepared", async () => {
  const { ipc } = await dropAndReject(commandErrors.model_busy, "/audio/updating.wav");
  ipc.handle("complete_update_preparation", () => undefined);
  ipc.handle("save_update_recovery", () => undefined);
  const retry = await screen.findByRole("button", { name: "Try again" });
  expect(retry).not.toHaveAttribute("aria-disabled", "true");

  ipc.emit("update-preparing", "nonce-1");

  await waitFor(() => expect(ipc.callsTo("complete_update_preparation")).toHaveLength(1));
  await waitFor(() => expect(retry).toHaveAttribute("aria-disabled", "true"));
  await userEvent.setup().click(retry);
  expect(ipc.callsTo("transcribe_file")).toHaveLength(1);

  // The update stops after preparation: the button is available again.
  ipc.emit("update-aborted", "Synthetic failure");
  await waitFor(() => expect(retry).not.toHaveAttribute("aria-disabled", "true"));
});
