// The Dictate tab offers a way out of an error by its code (ADR 0006, #360
// part 2b): "Try again" restarts the action (a new recording, never a second
// `stop_and_transcribe` of audio that is gone) when the backend says the
// action may be repeated, and "Download the model again" when the engine's
// model is missing or does not load. The text of the error never decides.
import { screen, waitFor, within } from "@testing-library/svelte";
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
    expect(button).not.toHaveAttribute("aria-disabled", "true");
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

    await waitFor(() => expect(tryAgain()).toHaveAttribute("aria-disabled", "true"));
  });
});

describe("Download the model again", () => {
  test.each([
    ["engine_model_missing", commandErrors.engine_model_missing],
    ["engine_model_load_failed", commandErrors.engine_model_load_failed],
  ])("is offered for %s instead of Try again", async (_code, failure) => {
    const { ipc } = await renderSettings(settingsBackend());
    ipc.emit("error", failure);

    expect(await screen.findByRole("button", { name: "Download the model again" })).not.toHaveAttribute("aria-disabled", "true");
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
    expect(screen.getByRole("button", { name: "Download the model again" })).toHaveAttribute("aria-disabled", "true");

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

  test.each([
    ["pianissimo_unavailable", backendError("pianissimo_unavailable", "Pianissimo is not available on this system.")],
    ["invalid_input", backendError("invalid_input", "That did not work.")],
  ])("a download refused with %s shows the error and offers nothing, so it cannot loop", async (_code, refusal) => {
    const { ipc } = await renderSettings(
      settingsBackend({
        download_pianissimo_model: () => {
          throw refusal;
        },
      }),
    );
    ipc.emit("error", commandErrors.engine_model_missing);

    await userEvent.setup().click(await screen.findByRole("button", { name: "Download the model again" }));

    await waitFor(() => expect(dictateError()?.textContent).toBe(refusal.message));
    await settle();
    expect(downloadAgain()).toBeNull();
    expect(tryAgain()).toBeNull();
    expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1);
  });

  test("a retryable failure of the download offers the download again, not a new recording", async () => {
    const { ipc } = await renderSettings(
      settingsBackend({
        download_pianissimo_model: () => {
          throw commandErrors.busy;
        },
      }),
    );
    ipc.emit("error", commandErrors.engine_model_missing);

    await userEvent.setup().click(await screen.findByRole("button", { name: "Download the model again" }));

    await waitFor(() => expect(dictateError()?.textContent).toBe(commandErrors.busy.message));
    expect(await screen.findByRole("button", { name: "Download the model again" })).toBeInTheDocument();
    expect(tryAgain()).toBeNull();
  });

  test("a new error with the very same text during the download keeps its own recovery", async () => {
    const download = deferred<void>();
    const { ipc } = await renderSettings(settingsBackend({ download_pianissimo_model: () => download.promise }));
    ipc.emit("error", backendError("engine_model_missing", "The model is not usable."));
    await userEvent.setup().click(await screen.findByRole("button", { name: "Download the model again" }));
    await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1));

    // The text is the same; the code is one nobody can repeat.
    ipc.emit("error", backendError("transcription_timed_out", "The model is not usable."));
    download.resolve();

    await settle();
    expect(dictateError()?.textContent).toBe("The model is not usable.");
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
    expect(await screen.findByRole("button", { name: "Download the model again" })).not.toHaveAttribute("aria-disabled", "true");
    expect(tryAgain()).toBeNull();
    await user.click(screen.getByRole("button", { name: "Download the model again" }));
    await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(2));
  });
});

// Focus must not fall to the page at a recovery transition: a keyboard or
// screen-reader user would lose their place (#360 review). The button is
// `aria-disabled` and guarded, not `disabled`, while a download runs, so it
// keeps the focus it has; one button carries both labels, so the focus stays on
// it when a finished download turns it into "Try again".
describe("focus", () => {
  test("Try again moves focus to the recording button it started", async () => {
    const { user } = await startAndStop(commandErrors.model_busy);

    await user.click(await screen.findByRole("button", { name: "Try again" }));

    const stop = await screen.findByRole("button", { name: "Stop recording" });
    await waitFor(() => expect(stop).toHaveFocus());
  });

  test("focus is on the recording button while the new recording is being started, never on the page", async () => {
    const start = deferred<void>();
    let starts = 0;
    const { user } = await startAndStop(commandErrors.model_busy, {
      start_recording: () => {
        starts += 1;
        return starts === 1 ? undefined : start.promise;
      },
    });
    const retry = await screen.findByRole("button", { name: "Try again" });

    await user.click(retry);

    // The second `start_recording` is still in flight, and the Try again button
    // is gone: the focus is already where the recording will show.
    await waitFor(() => expect(starts).toBe(2));
    expect(document.activeElement).not.toBe(document.body);
    expect(screen.getByRole("button", { name: "Start recording" })).toHaveFocus();
    start.resolve();
    expect(await screen.findByRole("button", { name: "Stop recording" })).toHaveFocus();
  });

  test("Try again that could not start a recording leaves focus on the Start button", async () => {
    let starts = 0;
    const { user } = await startAndStop(commandErrors.model_busy, {
      start_recording: () => {
        starts += 1;
        if (starts > 1) throw commandErrors.busy;
      },
    });
    // The second start fails.
    await user.click(await screen.findByRole("button", { name: "Try again" }));

    const start = await screen.findByRole("button", { name: "Start recording" });
    await waitFor(() => expect(start).toHaveFocus());
  });

  test("a download keeps focus on its button while it runs, and the Try again that follows has it", async () => {
    const download = deferred<void>();
    const { ipc } = await renderSettings(settingsBackend({ download_pianissimo_model: () => download.promise }));
    ipc.emit("error", commandErrors.engine_model_missing);
    const user = userEvent.setup();
    const button = await screen.findByRole("button", { name: "Download the model again" });

    await user.click(button);

    await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1));
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).not.toBeDisabled();
    expect(button).toHaveFocus();
    // Busy means busy: a second activation does nothing.
    await user.click(button);
    await user.keyboard("{Enter}");
    expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1);

    download.resolve();

    const retry = await screen.findByRole("button", { name: "Try again" });
    expect(retry).toHaveFocus();
    expect(retry).not.toHaveAttribute("aria-disabled", "true");
  });

  test("a download does not take focus from where the person moved meanwhile", async () => {
    const download = deferred<void>();
    const { ipc } = await renderSettings(settingsBackend({ download_pianissimo_model: () => download.promise }));
    ipc.emit("error", commandErrors.engine_model_missing);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Download the model again" }));
    await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1));
    const editor = screen.getByPlaceholderText("Click here and use your hotkey, or press the button above");
    editor.focus();

    download.resolve();

    await screen.findByRole("button", { name: "Try again" });
    expect(editor).toHaveFocus();
  });
});

// The error and the button that follows it are announced when they appear.
describe("announcement", () => {
  test("the Dictate tab's error is an alert", async () => {
    const { ipc } = await renderSettings(settingsBackend());

    ipc.emit("error", commandErrors.model_busy);

    const section = document.querySelector<HTMLElement>(".test-section")!;
    const alert = await within(section).findByRole("alert");
    expect(alert).toHaveTextContent(commandErrors.model_busy.message);
  });

  test("a restored plain-text error is an alert too", async () => {
    await renderSettings(settingsBackend({ get_last_error: () => "No audio was captured. Please try again." }));

    const section = document.querySelector<HTMLElement>(".test-section")!;
    expect(await within(section).findByRole("alert")).toHaveTextContent("No audio was captured. Please try again.");
  });
});
