// How Settings.svelte shows and branches on command errors (#360, ADR 0006):
// a typed error's `message` is shown as it is, a path-free Swift-style engine
// failure reaches the window without the path, and the dictionary conflict is
// recognised by its `code`, never by its text. Other Settings tests:
// Settings.test.ts, Settings.readback.test.ts, Settings.recovery.test.ts.
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import { backendError } from "../test/ipc";
import { renderSettings, settingsBackend } from "../test/settings";
import commandErrors from "../../contracts/transport/command_errors.json";

/** The error text of the Dictate tab's "Try it out" section, if shown. */
function dictateError(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".test-section .transcribe-error");
}

async function startAndStopRecording(stopFailure: unknown) {
  const { ipc } = await renderSettings(
    settingsBackend({
      start_recording: () => undefined,
      stop_and_transcribe: () => {
        throw stopFailure;
      },
    }),
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Start recording" }));
  await user.click(await screen.findByRole("button", { name: "Stop recording" }));
  return { ipc, user };
}

describe("an engine error from the Swift host", () => {
  // contracts/transport/command_errors.json is written by Rust from the real
  // serialization of `Failed to load model at /Users/x/Library/Application
  // Support/Sagascript/models/foo.mlmodelc: …` (command_error_tests.rs).
  const swift = commandErrors.engine_model_load_failed;

  test("the window shows it without the path the host reported", async () => {
    await startAndStopRecording(swift);
    await waitFor(() => expect(dictateError()).not.toBeNull());
    const shown = dictateError()!.textContent ?? "";
    expect(shown).toBe(swift.message);
    expect(shown).toContain("foo.mlmodelc");
    expect(shown).not.toMatch(/\/Users\/|\/home\/|[A-Za-z]:\\/);
    expect(document.body.textContent).not.toContain("/Users/x");
  });
});

describe("a failed recording", () => {
  test("a typed error shows its message", async () => {
    const { ipc } = await renderSettings(
      settingsBackend({
        start_recording: () => {
          throw commandErrors.busy;
        },
      }),
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Start recording" }));
    await waitFor(() => expect(dictateError()?.textContent).toBe(commandErrors.busy.message));
    expect(ipc.callsTo("start_recording")).toHaveLength(1);
  });
});

describe("the dictionary conflict", () => {
  async function saveStaleDictionary(failure: unknown) {
    const { ipc } = await renderSettings(
      settingsBackend({
        set_profile_glossary: () => {
          throw failure;
        },
      }),
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    await user.type(await screen.findByRole("textbox", { name: "Personal dictionary" }), "merge = merch");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(ipc.callsTo("set_profile_glossary")).toHaveLength(1));
    return { ipc, user };
  }
  const hint = /This dictionary changed elsewhere\. Your draft is preserved/;

  test("is recognised by its code, whatever the message says", async () => {
    await saveStaleDictionary(backendError("dictionary_changed_elsewhere", "The saved dictionary moved on."));
    expect(await screen.findByText(hint)).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Personal dictionary" })).toHaveValue("merge = merch");
  });

  test("the real wire message is shown as the error and starts the same recovery", async () => {
    await saveStaleDictionary(commandErrors.dictionary_changed_elsewhere);
    expect(await screen.findByText(hint)).toBeInTheDocument();
    expect(screen.getAllByText(commandErrors.dictionary_changed_elsewhere.message).length).toBeGreaterThan(0);
  });

  test("is not guessed from the message text of another error", async () => {
    await saveStaleDictionary(backendError("settings_storage_failed", "Dictionary changed elsewhere: only the words say so"));
    await screen.findAllByText("Dictionary changed elsewhere: only the words say so");
    expect(screen.queryByText(hint)).toBeNull();
  });
});

// `set_hotkey_profiles` (a profile-editor save) used to reject every refusal of
// the settings as `hotkey_invalid`. It now carries the code of the refusal
// (ADR 0006, #360 part 2b). Nothing in the window branches on `hotkey_invalid`
// or on any of these codes for a profile save, so the person sees the same
// text as before, once, and no other recovery opens.
describe("a refused profile-editor save", () => {
  const refusals = [
    backendError(
      "dictionary_blocks_language_change",
      "Profile 'default' has a personal dictionary; clear it before changing the profile language",
    ),
    backendError("model_incompatible", "Model 'Tiny (English)' is incompatible with profile 'default'"),
    backendError(
      "inactive_profile_dictionary",
      "Profile id 'default' has an inactive personal dictionary; choose a new id to avoid reactivating old aliases",
    ),
    backendError("profile_invalid", "Profile 'default' name must be 40 characters or fewer"),
    backendError("hotkey_invalid", "Duplicate hotkey 'Ctrl+Shift+Space'"),
  ];

  test.each(refusals)("$code shows its message and puts the language back", async (refusal) => {
    const { ipc } = await renderSettings(
      settingsBackend({
        set_hotkey_profiles: () => {
          throw refusal;
        },
      }),
    );
    const user = userEvent.setup();
    const language = await screen.findByRole("combobox", { name: "Svenska language" });

    await user.selectOptions(language, "English");

    await waitFor(() => expect(ipc.callsTo("set_hotkey_profiles")).toHaveLength(1));
    const shown = await screen.findAllByText(refusal.message);
    expect(shown.length).toBeGreaterThan(0);
    expect(language).toHaveValue("sv");
    // The window recovers from a blocked change of the default language (the
    // `set_language` setter); a profile save has no such panel.
    expect(screen.queryByRole("button", { name: /Clear default dictionary/ })).toBeNull();
    expect(ipc.callsTo("set_language")).toHaveLength(0);
  });
});
