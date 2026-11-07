// How Settings reacts to the dictation events of the backend (#362): the
// Dictate button and result follow `state-changed`, and an event wins over
// the restore of what the window missed before its listeners were active.
import { screen, waitFor } from "@testing-library/svelte";
import { describe, expect, test } from "vitest";
import { deferred } from "../test/deferred";
import { backendError } from "../test/ipc";
import { renderSettings, settingsBackend } from "../test/settings";
import commandErrors from "../../contracts/transport/command_errors.json";

const editor = () => screen.getByPlaceholderText("Click here and use your hotkey, or press the button above");

describe("state-changed", () => {
  test("a recording that starts while the last result is being restored wins over the restore", async () => {
    const lastResult = deferred<string | null>();
    const { ipc } = await renderSettings(
      settingsBackend({ get_last_transcription: () => lastResult.promise, get_last_error: () => "Gammalt fel" }),
      { restored: false },
    );
    await waitFor(() => expect(ipc.callsTo("get_last_transcription")).toHaveLength(1));

    ipc.emit("state-changed", "recording");
    lastResult.resolve("Gammal diktering");

    expect(await screen.findByRole("button", { name: "Recording via hotkey..." })).toBeInTheDocument();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(editor()).toHaveValue("");
    expect(screen.queryByText("Gammalt fel")).toBeNull();
  });

  test("without an event in between, the restore shows the last result and error", async () => {
    await renderSettings(settingsBackend({ get_last_transcription: () => "Gammal diktering", get_last_error: () => "Gammalt fel" }));

    await waitFor(() => expect(editor()).toHaveValue("Gammal diktering"));
    expect(screen.getByText("Gammalt fel")).toBeInTheDocument();
  });

  test("the Dictate button follows work the hotkey started", async () => {
    const { ipc } = await renderSettings(settingsBackend());

    ipc.emit("state-changed", "transcribing");
    expect(await screen.findByRole("button", { name: /Transcribing/ })).toBeDisabled();

    ipc.emit("state-changed", "idle");
    await waitFor(() => expect(screen.getByRole("button", { name: /Start recording/ })).toBeEnabled());
  });

  test("settings_reloaded re-reads the settings and every profile's models", async () => {
    const { ipc } = await renderSettings(settingsBackend());
    const before = ipc.calls.length;

    ipc.emit("state-changed", "settings_reloaded");

    await waitFor(() => expect(ipc.calls.slice(before).map((call) => call.command)).toContain("get_profile_model_info"));
    expect(ipc.calls.slice(before)[0].command).toBe("get_settings");
  });
});

describe("error", () => {
  // The hotkey path sends the typed error `{ code, message, retryable }` that a
  // failed command rejects with (ADR 0006, #360 part 2b); the window shows the
  // message as it showed the string before.
  test("a typed error from the hotkey path shows its message in the Dictate tab", async () => {
    const { ipc } = await renderSettings(settingsBackend());

    ipc.emit("error", backendError("transcription_failed", "Transcription failed: no model"));

    expect(await screen.findByText("Transcription failed: no model")).toBeInTheDocument();
    expect(document.querySelector(".test-section .transcribe-error")?.textContent).toBe("Transcription failed: no model");
  });

  test("the message of the real wire error reaches the window without a path", async () => {
    const { ipc } = await renderSettings(settingsBackend());

    ipc.emit("error", commandErrors.engine_model_missing);

    await waitFor(() =>
      expect(document.querySelector(".test-section .transcribe-error")?.textContent).toBe(commandErrors.engine_model_missing.message),
    );
    expect(document.body.textContent).not.toContain("/Users/");
  });

  test("a new successful result clears the error", async () => {
    const { ipc } = await renderSettings(settingsBackend());
    ipc.emit("error", backendError("busy", "Sagascript is busy.", true));
    await screen.findByText("Sagascript is busy.");

    ipc.emit("transcription-result", "Hej");

    await waitFor(() => expect(screen.queryByText("Sagascript is busy.")).toBeNull());
  });
});
