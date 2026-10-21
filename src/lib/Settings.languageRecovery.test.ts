// Settings.svelte: the recovery offered when a personal dictionary blocks a
// language change (#360). These tests exercise the rendered controls and the
// typed IPC boundary, so they cover both what the user sees and what is sent.
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import commandErrors from "../../contracts/transport/command_errors.json";
import type { HotkeyProfile, Settings as SettingsValue } from "./api";
import { backendError, type CommandHandlers } from "../test/ipc";
import { deferred, type Deferred } from "../test/deferred";
import { renderSettings, settingsBackend, settingsValue } from "../test/settings";

function profile(id: string, name: string, language: "sv" | "en", shortcut: string): HotkeyProfile {
  return { id, name, shortcut, language, push_to_talk_shortcut: shortcut, toggle_shortcut: null };
}

const blocked = (profileId: string) =>
  backendError(
    "dictionary_blocks_language_change",
    `Profile '${profileId}' has a personal dictionary; clear it before changing the profile language`,
  );

interface RecoveryBackendOptions {
  profiles?: HotkeyProfile[];
  dictionaries?: Record<string, string>;
  clearFailure?: string;
  retryFailure?: string;
  readFailureAfterRetry?: string;
  concurrentClearBeforePreview?: boolean;
  clearGate?: Deferred<void>;
  clearStarted?: Deferred<void>;
}

function recoveryBackend(options: RecoveryBackendOptions = {}) {
  const profiles = options.profiles ?? [
    profile("default", "Svenska", "sv", "Ctrl+Shift+Space"),
    profile("other", "Other", "en", "Ctrl+Alt+O"),
  ];
  const targetProfileId = profiles.find((item) => item.id === "default")?.id ?? profiles[0]?.id ?? "default";
  const targetLanguage = profiles.find((item) => item.id === targetProfileId)?.language ?? "sv";
  let stored: SettingsValue = settingsValue({
    language: targetLanguage,
    hotkey_profiles: profiles,
    profile_glossaries: options.dictionaries ?? { default: "merge = merch", other: "other = words" },
  });
  let languageCalls = 0;
  let failSettingsReads = false;
  let concurrentDictionaryWasCleared = false;

  const handlers: CommandHandlers = settingsBackend({
    get_settings: () => {
      if (options.concurrentClearBeforePreview && languageCalls === 1 && !concurrentDictionaryWasCleared) {
        stored = {
          ...stored,
          profile_glossaries: { ...stored.profile_glossaries, [targetProfileId]: "" },
        };
        concurrentDictionaryWasCleared = true;
      }
      if (failSettingsReads) {
        throw backendError("unavailable", options.readFailureAfterRetry ?? "settings read failed");
      }
      return structuredClone(stored);
    },
    set_language: ({ language }) => {
      languageCalls += 1;
      const currentProfile = stored.hotkey_profiles.find((item) => item.id === targetProfileId);
      const currentLanguage = currentProfile?.language ?? stored.language;
      const currentDictionary = stored.profile_glossaries[targetProfileId] ?? "";
      if (currentDictionary.trim() && currentLanguage !== language) throw blocked(targetProfileId);
      if (options.retryFailure && languageCalls > 1 && currentLanguage !== language) {
        throw backendError("settings_storage_failed", options.retryFailure);
      }

      const changed = currentLanguage !== language;
      stored = {
        ...stored,
        language,
        hotkey_profiles: stored.hotkey_profiles.map((item) => (item.id === targetProfileId ? { ...item, language } : item)),
      };
      if (options.readFailureAfterRetry && languageCalls > 1 && changed) failSettingsReads = true;
      return [];
    },
    set_profile_glossary: async ({ profileId, source, expectedSource }) => {
      options.clearStarted?.resolve();
      await options.clearGate?.promise;
      if (options.clearFailure) throw backendError("settings_storage_failed", options.clearFailure);
      const currentSource = stored.profile_glossaries[profileId] ?? "";
      if (expectedSource !== currentSource) {
        throw backendError("dictionary_changed_elsewhere", `Dictionary changed elsewhere: ${profileId}`);
      }
      stored = {
        ...stored,
        profile_glossaries: { ...stored.profile_glossaries, [profileId]: source },
      };
    },
  });

  return {
    handlers,
    current: () => structuredClone(stored),
    setDictionary(profileId: string, source: string) {
      stored = { ...stored, profile_glossaries: { ...stored.profile_glossaries, [profileId]: source } };
    },
  };
}

async function openSettingsTab() {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Settings" }));
  return user;
}

async function requestBlockedChange(user: ReturnType<typeof userEvent.setup>) {
  const language = screen.getByRole("combobox", { name: "Default profile language" });
  await user.selectOptions(language, "English");
  const preview = (await screen.findByLabelText("Saved default profile dictionary")) as HTMLTextAreaElement;
  return { language, preview };
}

describe("a language change a personal dictionary blocks", () => {
  test("the backend fixture names this case by code, as non-retryable, with a dictionary explanation", () => {
    const fixture = commandErrors.dictionary_blocks_language_change;
    expect(fixture.code).toBe("dictionary_blocks_language_change");
    expect(fixture.retryable).toBe(false);
    expect(fixture.message).toMatch(/personal dictionary/);
  });

  test("rejection restores the selection, previews the target dictionary, and Cancel prevents clearing", async () => {
    const fixture = recoveryBackend();
    const { ipc } = await renderSettings(fixture.handlers);
    const user = await openSettingsTab();
    await user.selectOptions(screen.getByRole("combobox", { name: "" }), "other");
    const dictionary = screen.getByLabelText("Personal dictionary") as HTMLTextAreaElement;
    expect(dictionary).toHaveValue("other = words");

    const { language, preview } = await requestBlockedChange(user);

    expect(language).toHaveDisplayValue("Swedish");
    expect(preview).toHaveValue("merge = merch");
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    expect(screen.queryByLabelText("Saved default profile dictionary")).toBeNull();
    expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]);
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);
    expect(fixture.current().language).toBe("sv");
  });

  test("clears only the language target and preserves a dirty draft in another scope", async () => {
    const fixture = recoveryBackend({
      profiles: [profile("work", "Arbete", "sv", "Ctrl+Alt+W"), profile("other", "Other", "en", "Ctrl+Alt+O")],
      dictionaries: { work: "merge = merch", other: "other = words" },
    });
    const { ipc } = await renderSettings(fixture.handlers);
    const user = await openSettingsTab();
    await user.selectOptions(screen.getByRole("combobox", { name: "" }), "other");
    const dictionary = screen.getByLabelText("Personal dictionary") as HTMLTextAreaElement;
    await user.clear(dictionary);
    await user.type(dictionary, "other unsaved draft");
    const firstMutation = ipc.calls.length;

    const { preview } = await requestBlockedChange(user);
    expect(preview).toHaveValue("merge = merch");
    await user.click(screen.getByRole("button", { name: "Clear default dictionary and switch to English" }));

    await waitFor(() => expect(fixture.current().language).toBe("en"));
    const recoveryCalls = ipc.calls
      .slice(firstMutation)
      .filter((call) => call.command === "set_language" || call.command === "set_profile_glossary");
    expect(recoveryCalls).toEqual([
      { command: "set_language", args: { language: "en" } },
      { command: "set_profile_glossary", args: { profileId: "work", source: "", expectedSource: "merge = merch" } },
      { command: "set_language", args: { language: "en" } },
    ]);
    expect(fixture.current().profile_glossaries).toEqual({ work: "", other: "other = words" });
    expect(screen.getByDisplayValue("Other · English")).toBeInTheDocument();
    expect(dictionary).toHaveValue("other unsaved draft");
  });

  test("uses the profile named default even when it is not first", async () => {
    const fixture = recoveryBackend({
      profiles: [profile("work", "Arbete", "sv", "Ctrl+Alt+W"), profile("default", "Svenska", "sv", "Ctrl+Shift+Space")],
      dictionaries: { work: "work = words", default: "merge = merch" },
    });
    const { ipc } = await renderSettings(fixture.handlers);
    const user = await openSettingsTab();

    await requestBlockedChange(user);
    await user.click(screen.getByRole("button", { name: "Clear default dictionary and switch to English" }));

    await waitFor(() => expect(fixture.current().language).toBe("en"));
    expect(ipc.callsTo("set_profile_glossary")).toEqual([{ profileId: "default", source: "", expectedSource: "merge = merch" }]);
    expect(fixture.current().profile_glossaries).toEqual({ work: "work = words", default: "" });
  });

  test("recovers a dirty default-profile draft after the saved dictionary is cleared", async () => {
    const fixture = recoveryBackend({ dictionaries: { default: "saved words", other: "other = words" } });
    const { ipc } = await renderSettings(fixture.handlers);
    const user = await openSettingsTab();
    const dictionary = screen.getByLabelText("Personal dictionary") as HTMLTextAreaElement;
    await user.clear(dictionary);
    await user.type(dictionary, "default unsaved draft");
    await requestBlockedChange(user);
    await user.click(screen.getByRole("button", { name: "Clear default dictionary and switch to English" }));

    await waitFor(() => expect(fixture.current().language).toBe("en"));
    expect(ipc.callsTo("set_profile_glossary")).toEqual([{ profileId: "default", source: "", expectedSource: "saved words" }]);
    expect(dictionary).toHaveValue("");
    expect(screen.getByLabelText("Unsaved draft for Svenska")).toHaveValue("default unsaved draft");
  });

  test("a changed dictionary fails the expectedSource comparison without retrying the language", async () => {
    const fixture = recoveryBackend({ dictionaries: { default: "original words", other: "other = words" } });
    const { ipc } = await renderSettings(fixture.handlers);
    const user = await openSettingsTab();
    await requestBlockedChange(user);
    fixture.setDictionary("default", "changed elsewhere");

    await user.click(screen.getByRole("button", { name: "Clear default dictionary and switch to English" }));
    expect(await screen.findByText(/The dictionary could not be cleared; the language was not changed/)).toBeInTheDocument();

    expect(ipc.callsTo("set_profile_glossary")).toEqual([{ profileId: "default", source: "", expectedSource: "original words" }]);
    expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]);
    expect(fixture.current().profile_glossaries.default).toBe("changed elsewhere");
    expect(fixture.current().language).toBe("sv");
  });

  test("a clear persistence failure preserves the saved dictionary and dirty draft", async () => {
    const fixture = recoveryBackend({
      dictionaries: { default: "saved words", other: "other = words" },
      clearFailure: "disk full",
    });
    const { ipc } = await renderSettings(fixture.handlers);
    const user = await openSettingsTab();
    const dictionary = screen.getByLabelText("Personal dictionary") as HTMLTextAreaElement;
    await user.clear(dictionary);
    await user.type(dictionary, "keep this draft");
    await requestBlockedChange(user);
    await user.click(screen.getByRole("button", { name: "Clear default dictionary and switch to English" }));

    expect(await screen.findByText(/The dictionary could not be cleared; the language was not changed: disk full/)).toBeInTheDocument();
    expect(fixture.current().profile_glossaries.default).toBe("saved words");
    expect(fixture.current().language).toBe("sv");
    expect(dictionary).toHaveValue("keep this draft");
    expect(screen.queryByLabelText("Unsaved draft for Svenska")).toBeNull();
    expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]);
    expect(ipc.callsTo("set_profile_glossary")).toEqual([{ profileId: "default", source: "", expectedSource: "saved words" }]);
  });

  test("a failed language retry shows the completed clear and preserves copyable words without offering a repeat", async () => {
    const fixture = recoveryBackend({
      dictionaries: { default: "saved words", other: "other = words" },
      retryFailure: "language retry failed",
    });
    const { ipc } = await renderSettings(fixture.handlers);
    const user = await openSettingsTab();
    await requestBlockedChange(user);
    await user.click(screen.getByRole("button", { name: "Clear default dictionary and switch to English" }));

    expect(
      await screen.findByText(/The default dictionary was cleared, but the language change failed: language retry failed/),
    ).toBeInTheDocument();
    expect(fixture.current().profile_glossaries.default).toBe("");
    expect(fixture.current().language).toBe("sv");
    expect(screen.getByLabelText("Unsaved draft for Svenska")).toHaveValue("saved words");
    expect(screen.queryByRole("button", { name: /Clear default dictionary and switch/ })).toBeNull();
    expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }, { language: "en" }]);
    expect(ipc.callsTo("set_profile_glossary")).toHaveLength(1);
  });

  test("a failed read after clear and language success reports saved and does not offer another clear", async () => {
    const fixture = recoveryBackend({
      dictionaries: { default: "saved words", other: "other = words" },
      readFailureAfterRetry: "read after recovery failed",
    });
    const { ipc } = await renderSettings(fixture.handlers);
    const user = await openSettingsTab();
    await requestBlockedChange(user);
    await user.click(screen.getByRole("button", { name: "Clear default dictionary and switch to English" }));

    const notice = await screen.findByRole("alert", { name: "Settings not confirmed" });
    expect(notice).toHaveTextContent(/Your change was saved, but could not be read back to confirm it/);
    expect(notice).toHaveTextContent("read after recovery failed");
    expect(fixture.current().profile_glossaries.default).toBe("");
    expect(fixture.current().language).toBe("en");
    expect(screen.queryByRole("button", { name: /Clear default dictionary and switch/ })).toBeNull();
    expect(screen.getByRole("combobox", { name: "Default profile language" })).toBeDisabled();
    expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }, { language: "en" }]);
    expect(ipc.callsTo("set_profile_glossary")).toHaveLength(1);
  });

  test("duplicate clear and language actions are blocked while the compare-and-swap is pending", async () => {
    const clearGate = deferred<void>();
    const clearStarted = deferred<void>();
    const fixture = recoveryBackend({ clearGate, clearStarted });
    const { ipc } = await renderSettings(fixture.handlers);
    const user = await openSettingsTab();
    await requestBlockedChange(user);
    const clear = screen.getByRole("button", { name: "Clear default dictionary and switch to English" });
    const language = screen.getByRole("combobox", { name: "Default profile language" });

    await user.click(clear);
    await clearStarted.promise;
    await waitFor(() => expect(clear).toBeDisabled());
    expect(language).toBeDisabled();
    await user.click(clear);
    await user.selectOptions(language, "Norwegian");
    expect(ipc.callsTo("set_profile_glossary")).toHaveLength(1);
    expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]);

    clearGate.resolve();
    await waitFor(() => expect(fixture.current().language).toBe("en"));
    expect(ipc.callsTo("set_profile_glossary")).toHaveLength(1);
    expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }, { language: "en" }]);
  });

  test("an empty dictionary allows an ordinary language change without recovery", async () => {
    const fixture = recoveryBackend({ dictionaries: { default: "", other: "other = words" } });
    const { ipc } = await renderSettings(fixture.handlers);
    const user = await openSettingsTab();
    const language = screen.getByRole("combobox", { name: "Default profile language" });

    await user.selectOptions(language, "English");

    expect(language).toHaveDisplayValue("English");
    expect(fixture.current().language).toBe("en");
    expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]);
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);
    expect(screen.queryByLabelText("Saved default profile dictionary")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  test("a dictionary cleared before the preview produces retry guidance", async () => {
    const fixture = recoveryBackend({ concurrentClearBeforePreview: true });
    const { ipc } = await renderSettings(fixture.handlers);
    const user = await openSettingsTab();
    const language = screen.getByRole("combobox", { name: "Default profile language" });

    await user.selectOptions(language, "English");

    expect(await screen.findByText(/blocking dictionary was cleared elsewhere.*Select the language again to retry/)).toBeInTheDocument();
    expect(language).toHaveDisplayValue("Swedish");
    expect(fixture.current().profile_glossaries.default).toBe("");
    expect(fixture.current().language).toBe("sv");
    expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]);
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);
    expect(screen.queryByLabelText("Saved default profile dictionary")).toBeNull();
  });
});
