// Rendered coverage for profile-owned dictionaries and file transcription profiles (#361).
import { emit } from "@tauri-apps/api/event";
import { screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import type { HotkeyProfile, Settings as SettingsValue } from "./api";
import { deferred } from "../test/deferred";
import { backendError } from "../test/ipc";
import { renderSettings, settingsBackend, settingsValue, swedishProfile } from "../test/settings";

const workProfile: HotkeyProfile = {
  ...swedishProfile,
  id: "work",
  name: "Work",
  language: "fi",
  shortcut: "Ctrl+Alt+W",
};

const globalProfile: HotkeyProfile = {
  ...swedishProfile,
  id: "global",
  name: "Global",
  language: "en",
  shortcut: "Ctrl+Alt+G",
};

function dictionaryScope(): HTMLSelectElement {
  // Settings currently leaves this select without an accessible name. Its
  // native select role is still exposed, and the visible options identify it.
  const scope = screen
    .getAllByRole("combobox")
    .find((control) => within(control).queryAllByRole("option", { name: /Svenska · Swedish|Work · Finnish|Global · English/ }).length > 0);
  if (!(scope instanceof HTMLSelectElement)) throw new Error("Dictionary scope select was not rendered");
  return scope;
}

async function openSettingsTab() {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Settings" }));
  return user;
}

describe("profile dictionary scopes", () => {
  test("only profiles are scopes, switching is local, and a profile named global saves under its id", async () => {
    let persisted = settingsValue({
      hotkey_profiles: [swedishProfile, workProfile, globalProfile],
      profile_glossaries: {
        default: "Default words",
        work: "Work words",
        global: "Global words",
      },
    });
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => persisted,
        set_profile_glossary: ({ profileId, source }) => {
          persisted = {
            ...persisted,
            profile_glossaries: { ...persisted.profile_glossaries, [profileId]: source },
          };
        },
      }),
    );
    const user = await openSettingsTab();
    const scope = dictionaryScope();
    const dictionary = screen.getByRole("textbox", { name: "Personal dictionary" });

    expect(
      within(scope)
        .getAllByRole("option")
        .map((option) => option.textContent?.trim()),
    ).toEqual(["Svenska · Swedish", "Work · Finnish", "Global · English"]);
    expect(dictionary).toHaveValue("Default words");

    await user.selectOptions(scope, "work");
    expect(dictionary).toHaveValue("Work words");
    await user.selectOptions(scope, "global");
    expect(dictionary).toHaveValue("Global words");
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);
    expect(ipc.callsTo("set_initial_prompt")).toEqual([]);

    await user.clear(dictionary);
    await user.type(dictionary, "Updated global words");
    await user.tab();
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() =>
      expect(ipc.callsTo("set_profile_glossary")).toEqual([
        { profileId: "global", source: "Updated global words", expectedSource: "Global words" },
      ]),
    );
    expect(ipc.callsTo("set_initial_prompt")).toEqual([]);
  });

  test("clean scopes follow external refreshes while an edited draft stays visible", async () => {
    let persisted = settingsValue({ profile_glossaries: { default: "Saved v1" } });
    const { ipc } = await renderSettings(settingsBackend({ get_settings: () => persisted }));
    const user = await openSettingsTab();
    const dictionary = screen.getByRole("textbox", { name: "Personal dictionary" });

    persisted = settingsValue({ profile_glossaries: { default: "Saved v2" } });
    ipc.emit("state-changed", "settings_reloaded");
    await waitFor(() => expect(dictionary).toHaveValue("Saved v2"));

    await user.clear(dictionary);
    await user.type(dictionary, "My unsaved draft");
    persisted = settingsValue({ language: "en", profile_glossaries: { default: "Saved v3" } });
    ipc.emit("state-changed", "settings_reloaded");

    await waitFor(() => expect(screen.getByRole("combobox", { name: "Default profile language" })).toHaveValue("en"));
    expect(dictionary).toHaveValue("My unsaved draft");
    expect(screen.getByRole("button", { name: "Save changes" })).toBeEnabled();
  });

  test("a stale compare-and-swap conflict reloads the source for a later save", async () => {
    let persisted = settingsValue({
      hotkey_profiles: [swedishProfile, workProfile],
      profile_glossaries: { default: "Saved before conflict", work: "Work dictionary" },
    });
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => persisted,
        set_profile_glossary: ({ profileId, source, expectedSource }) => {
          const current = persisted.profile_glossaries[profileId] ?? "";
          if (expectedSource !== current) {
            throw backendError("dictionary_changed_elsewhere", "The saved dictionary moved on.");
          }
          persisted = {
            ...persisted,
            profile_glossaries: { ...persisted.profile_glossaries, [profileId]: source },
          };
        },
      }),
    );
    const user = await openSettingsTab();
    const scope = dictionaryScope();
    const dictionary = screen.getByRole("textbox", { name: "Personal dictionary" });

    await user.clear(dictionary);
    await user.type(dictionary, "Draft from the old source");
    persisted = settingsValue({
      language: "en",
      hotkey_profiles: [{ ...swedishProfile, language: "en" }, workProfile],
      profile_glossaries: { default: "Changed elsewhere", work: "Work dictionary" },
    });
    ipc.emit("state-changed", "settings_reloaded");
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Default profile language" })).toHaveValue("en"));
    expect(dictionary).toHaveValue("Draft from the old source");
    // This third value is introduced after the UI refresh and without another
    // event. Only the conflict handler's own get_settings read can discover it.
    persisted = settingsValue({
      language: "en",
      hotkey_profiles: [{ ...swedishProfile, language: "en" }, workProfile],
      profile_glossaries: { default: "Latest source at conflict", work: "Work dictionary" },
    });

    await user.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByText(/This dictionary changed elsewhere\. Your draft is preserved/)).toBeInTheDocument();
    expect(ipc.callsTo("set_profile_glossary")[0]).toEqual({
      profileId: "default",
      source: "Draft from the old source",
      expectedSource: "Saved before conflict",
    });
    expect(dictionary).toHaveValue("Draft from the old source");

    await user.selectOptions(scope, "work");
    await user.click(screen.getByRole("button", { name: "Discard" }));
    await user.selectOptions(scope, "default");
    expect(dictionary).toHaveValue("Latest source at conflict");

    await user.clear(dictionary);
    await user.type(dictionary, "Retry from refreshed source");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(ipc.callsTo("set_profile_glossary")).toHaveLength(2));
    expect(ipc.callsTo("set_profile_glossary")[1]).toEqual({
      profileId: "default",
      source: "Retry from refreshed source",
      expectedSource: "Latest source at conflict",
    });
  });

  test("a non-conflict write failure keeps the draft and the original baseline for retry", async () => {
    let persisted = settingsValue({ profile_glossaries: { default: "Stored source" } });
    let refuseFirstWrite = true;
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => persisted,
        set_profile_glossary: ({ profileId, source }) => {
          if (refuseFirstWrite) {
            refuseFirstWrite = false;
            throw backendError("settings_storage_failed", "Disk is full.");
          }
          persisted = {
            ...persisted,
            profile_glossaries: { ...persisted.profile_glossaries, [profileId]: source },
          };
        },
      }),
    );
    const user = await openSettingsTab();
    const dictionary = screen.getByRole("textbox", { name: "Personal dictionary" });

    await user.clear(dictionary);
    await user.type(dictionary, "Draft to retry");
    await user.tab();
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByText("Disk is full.")).toBeInTheDocument();
    expect(dictionary).toHaveValue("Draft to retry");
    expect(screen.getByRole("button", { name: "Save changes" })).toBeEnabled();

    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(ipc.callsTo("set_profile_glossary")).toHaveLength(2));
    expect(ipc.callsTo("set_profile_glossary")).toEqual([
      { profileId: "default", source: "Draft to retry", expectedSource: "Stored source" },
      { profileId: "default", source: "Draft to retry", expectedSource: "Stored source" },
    ]);
  });

  test("external profile removal during a pending save preserves the removed scope draft", async () => {
    let persisted = settingsValue({
      hotkey_profiles: [swedishProfile, workProfile],
      profile_glossaries: { default: "Default source", work: "Work source" },
    });
    const save = deferred<void>();
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => persisted,
        set_profile_glossary: () => save.promise,
      }),
    );
    const user = await openSettingsTab();
    const scope = dictionaryScope();
    const dictionary = screen.getByRole("textbox", { name: "Personal dictionary" });

    await user.clear(dictionary);
    await user.type(dictionary, "Draft from removed profile");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(ipc.callsTo("set_profile_glossary")).toHaveLength(1));
    expect(scope).toBeDisabled();
    expect(dictionary).toBeDisabled();

    persisted = settingsValue({
      hotkey_profiles: [workProfile],
      profile_glossaries: { work: "Work source" },
    });
    ipc.emit("state-changed", "settings_reloaded");
    await waitFor(() => expect(scope).toHaveValue("work"));
    expect(dictionary).toHaveValue("Work source");

    save.reject(backendError("settings_storage_failed", "The removed profile could not be saved."));
    const recovery = await screen.findByRole("textbox", { name: "Unsaved draft for default" });
    await waitFor(() => {
      expect(scope).toBeEnabled();
      expect(dictionary).toBeEnabled();
    });
    expect(recovery).toHaveValue("Draft from removed profile");
    expect(scope).toHaveValue("work");
    expect(dictionary).toHaveValue("Work source");
    expect(ipc.callsTo("set_profile_glossary")[0]).toEqual({
      profileId: "default",
      source: "Draft from removed profile",
      expectedSource: "Default source",
    });
  });

  test("a late compare-and-swap conflict annotates a recovered draft after its profile is removed", async () => {
    let persisted = settingsValue({
      hotkey_profiles: [swedishProfile, workProfile],
      profile_glossaries: { default: "Default source", work: "Work source" },
    });
    const save = deferred<void>();
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => persisted,
        set_profile_glossary: () => save.promise,
      }),
    );
    const user = await openSettingsTab();
    const scope = dictionaryScope();
    const dictionary = screen.getByRole("textbox", { name: "Personal dictionary" });

    await user.clear(dictionary);
    await user.type(dictionary, "Draft from removed profile");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(ipc.callsTo("set_profile_glossary")).toHaveLength(1));

    // External settings reload removes the saved scope before its write rejects.
    persisted = settingsValue({
      hotkey_profiles: [workProfile],
      profile_glossaries: { work: "Work source" },
    });
    ipc.emit("state-changed", "settings_reloaded");
    await waitFor(() => expect(scope).toHaveValue("work"));
    const recovery = await screen.findByRole("textbox", { name: "Unsaved draft for default" });
    expect(recovery).toHaveValue("Draft from removed profile");
    expect(screen.queryByText(/the saved dictionary changed elsewhere/i)).not.toBeInTheDocument();

    save.reject(backendError("dictionary_changed_elsewhere", "The saved dictionary moved on."));
    await waitFor(() => {
      expect(scope).toBeEnabled();
      expect(dictionary).toBeEnabled();
    });

    expect(scope).toHaveValue("work");
    expect(dictionary).toHaveValue("Work source");
    expect(recovery).toHaveValue("Draft from removed profile");
    await waitFor(() => expect(recovery.parentElement).toHaveTextContent(/the saved dictionary changed elsewhere/i));
  });
});

test("file transcription uses the selected profile for both language display and its dictionary", async () => {
  const snapshot: SettingsValue = settingsValue({
    hotkey_profiles: [workProfile, swedishProfile],
    profile_glossaries: { default: "Swedish dictionary", work: "Finnish dictionary" },
  });
  const { ipc } = await renderSettings(
    settingsBackend({
      get_settings: () => snapshot,
      transcribe_file: ({ profileId }) => `Transcript for ${profileId}`,
      set_update_result_pending: () => undefined,
    }),
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Transcribe" }));
  const profile = screen.getByRole("combobox", { name: "Profile (optional)" });
  const context = screen.getByRole("textbox", { name: "Extra context for this file" });
  await user.type(context, "  Astrid, Grimnir  ");

  expect(profile).toHaveValue("default");
  expect(context).toHaveValue("  Astrid, Grimnir  ");
  expect(screen.getByRole("button", { name: /Language Swedish/ })).toBeInTheDocument();
  expect(screen.getByText("This profile fixes the file language and uses its personal dictionary.")).toBeInTheDocument();
  await emit("tauri://drag-drop", { paths: ["/audio/swedish.wav"], position: { x: 0, y: 0 } });
  await screen.findByDisplayValue("Transcript for default");
  expect(ipc.callsTo("transcribe_file")[0]).toMatchObject({
    profileId: "default",
    prompt: "Astrid, Grimnir",
    autoPaste: false,
  });

  await waitFor(() => expect(profile).toBeEnabled());
  await user.selectOptions(profile, "work");
  expect(screen.getByRole("button", { name: /Language Finnish/ })).toBeInTheDocument();
  await emit("tauri://drag-drop", { paths: ["/audio/finnish.wav"], position: { x: 0, y: 0 } });
  await screen.findByDisplayValue("Transcript for work");
  expect(ipc.callsTo("transcribe_file")).toEqual([
    expect.objectContaining({ profileId: "default", prompt: "Astrid, Grimnir", autoPaste: false }),
    expect.objectContaining({ profileId: "work", prompt: "Astrid, Grimnir", autoPaste: false }),
  ]);
});
