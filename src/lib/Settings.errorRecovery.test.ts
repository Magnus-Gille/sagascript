// The Dictate tab offers a way out of an error by its code (ADR 0006, #360
// part 2b): "Try again" restarts the action (a new recording, never a second
// `stop_and_transcribe` of audio that is gone) when the backend says the
// action may be repeated, and "Download the model again" when the engine's
// model is missing or does not load. The text of the error never decides.
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import { deferred, settle } from "../test/deferred";
import { backendError } from "../test/ipc";
import { renderSettings, settingsBackend } from "../test/settings";
import commandErrors from "../../contracts/transport/command_errors.json";

const tryAgain = () => screen.queryByRole("button", { name: "Try again" });
const downloadAgain = () => screen.queryByRole("button", { name: "Download the model again" });
const dictateError = () => document.querySelector<HTMLElement>(".test-section .transcribe-error");

async function startAndStop(stopFailure: unknown, extra = {}) {
  const rendered = await renderSettings(
    settingsBackend({
      start_recording: () => undefined,
      stop_and_transcribe: () => {
        throw stopFailure;
      },
      ...extra,
    }),
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Start recording" }));
  await user.click(await screen.findByRole("button", { name: "Stop recording" }));
  return { ...rendered, user };
}

describe("Try again", () => {
  test("after a failed transcription it starts a new recording and never repeats the stop", async () => {
    const { ipc, user } = await startAndStop(commandErrors.model_busy);
    await waitFor(() => expect(dictateError()?.textContent).toBe(commandErrors.model_busy.message));
    expect(ipc.callsTo("start_recording")).toHaveLength(1);
    expect(ipc.callsTo("stop_and_transcribe")).toHaveLength(1);

    await user.click(await screen.findByRole("button", { name: "Try again" }));

    await waitFor(() => expect(ipc.callsTo("start_recording")).toHaveLength(2));
    // The audio of the first recording is consumed: repeating the stop would
    // resolve with an empty text (ADR 0006 decision 6).
    expect(ipc.callsTo("stop_and_transcribe")).toHaveLength(1);
    expect(await screen.findByRole("button", { name: "Stop recording" })).toBeInTheDocument();
    expect(dictateError()).toBeNull();
    expect(tryAgain()).toBeNull();
  });

  test("is offered for the error event of a hotkey dictation, decided by its retry hint and not by its text", async () => {
    const { ipc } = await renderSettings(settingsBackend({ start_recording: () => undefined }));
    // A text that says nothing about trying again, from a retryable code.
    ipc.emit("error", backendError("engine_busy", "Pianissimo engine: queue full", true));

    const button = await screen.findByRole("button", { name: "Try again" });
    expect(button).toBeEnabled();
    expect(dictateError()?.textContent).toBe("Pianissimo engine: queue full");
    expect(downloadAgain()).toBeNull();

    await userEvent.setup().click(button);
    await waitFor(() => expect(ipc.callsTo("start_recording")).toHaveLength(1));
  });

  test("is offered after a recording that could not start because Sagascript is busy", async () => {
    const { ipc } = await renderSettings(
      settingsBackend({
        start_recording: () => {
          throw commandErrors.busy;
        },
      }),
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Start recording" }));

    await user.click(await screen.findByRole("button", { name: "Try again" }));

    await waitFor(() => expect(ipc.callsTo("start_recording")).toHaveLength(2));
  });

  test.each([
    ["transcription_timed_out", commandErrors.transcription_timed_out],
    ["cancelled", commandErrors.cancelled],
    ["paste_error", commandErrors.paste_error],
    ["unknown_profile", commandErrors.unknown_profile],
    // The text promises a retry; the code and the hint do not.
    ["invalid_input", backendError("invalid_input", "That did not work. Please try again.")],
  ])("is not offered for %s", async (_code, failure) => {
    const { ipc } = await renderSettings(settingsBackend());
    ipc.emit("error", failure);

    await waitFor(() => expect(dictateError()?.textContent).toBe(failure.message));
    expect(tryAgain()).toBeNull();
    expect(downloadAgain()).toBeNull();
  });

  test("is not offered for the plain text of the last error the window restores", async () => {
    await renderSettings(settingsBackend({ get_last_error: () => "No audio was captured. Please try again." }));

    await waitFor(() => expect(dictateError()?.textContent).toBe("No audio was captured. Please try again."));
    expect(tryAgain()).toBeNull();
  });

  test("goes away with the error when a result arrives", async () => {
    const { ipc } = await renderSettings(settingsBackend());
    ipc.emit("error", commandErrors.no_audio_captured);
    await screen.findByRole("button", { name: "Try again" });

    ipc.emit("transcription-result", "Hej");

    await waitFor(() => expect(tryAgain()).toBeNull());
    expect(dictateError()).toBeNull();
  });

  test("waits while another recording is going on", async () => {
    const { ipc } = await renderSettings(settingsBackend());
    ipc.emit("error", commandErrors.model_busy);
    await screen.findByRole("button", { name: "Try again" });

    ipc.emit("state-changed", "transcribing");

    await waitFor(() => expect(tryAgain()).toBeDisabled());
  });
});

describe("Download the model again", () => {
  test.each([
    ["engine_model_missing", commandErrors.engine_model_missing],
    ["engine_model_load_failed", commandErrors.engine_model_load_failed],
  ])("is offered for %s instead of Try again", async (_code, failure) => {
    const { ipc } = await renderSettings(settingsBackend());
    ipc.emit("error", failure);

    expect(await screen.findByRole("button", { name: "Download the model again" })).toBeEnabled();
    expect(dictateError()?.textContent).toBe(failure.message);
    expect(tryAgain()).toBeNull();
  });

  test("downloads the engine's model, then offers Try again", async () => {
    const download = deferred<void>();
    const { ipc } = await renderSettings(
      settingsBackend({ start_recording: () => undefined, download_pianissimo_model: () => download.promise }),
    );
    ipc.emit("error", commandErrors.engine_model_missing);
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Download the model again" }));

    await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1));
    expect(ipc.callsTo("download_model")).toHaveLength(0);
    // The progress the window shows for every download, and no second click.
    await waitFor(() => expect(document.querySelector(".download-status-label")?.textContent).toBe("Downloading Pianissimo..."));
    expect(screen.getByRole("button", { name: "Download the model again" })).toBeDisabled();

    download.resolve();

    const retry = await screen.findByRole("button", { name: "Try again" });
    expect(downloadAgain()).toBeNull();
    await user.click(retry);
    await waitFor(() => expect(ipc.callsTo("start_recording")).toHaveLength(1));
  });

  test("a different error that arrives during the download keeps its own recovery", async () => {
    const download = deferred<void>();
    const { ipc } = await renderSettings(settingsBackend({ download_pianissimo_model: () => download.promise }));
    ipc.emit("error", commandErrors.engine_model_missing);
    await userEvent.setup().click(await screen.findByRole("button", { name: "Download the model again" }));
    await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1));

    // A dictation by hotkey fails meanwhile, with an error nobody can repeat.
    ipc.emit("error", commandErrors.transcription_timed_out);
    download.resolve();

    await waitFor(() => expect(dictateError()?.textContent).toBe(commandErrors.transcription_timed_out.message));
    await settle();
    expect(tryAgain()).toBeNull();
    expect(downloadAgain()).toBeNull();
  });

  test("a failed download shows its error and can be tried again", async () => {
    const { ipc } = await renderSettings(
      settingsBackend({
        download_pianissimo_model: () => {
          throw backendError("model_download_failed", "Model download failed: offline");
        },
      }),
    );
    ipc.emit("error", commandErrors.engine_model_missing);
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Download the model again" }));

    await waitFor(() => expect(dictateError()?.textContent).toBe("Model download failed: offline"));
    expect(await screen.findByRole("button", { name: "Download the model again" })).toBeEnabled();
    expect(tryAgain()).toBeNull();
    await user.click(screen.getByRole("button", { name: "Download the model again" }));
    await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(2));
  });
});
