// Settings.svelte: the recovery offered when a personal dictionary blocks a
// language change (#360). The backend names the case by its code
// (`dictionary_blocks_language_change`) and not by the profile: `set_language`
// changes the profile called "default", else the first one
// (HotkeyProfile::default_index in sagascript-core), and that profile's
// dictionary is the one to preview, clear and remember. Not necessarily the
// profile with the id "default": the profile can be removed.
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import type { HotkeyProfile } from "./api";
import { backendError } from "../test/ipc";
import { renderSettings, settingsBackend, settingsValue } from "../test/settings";

function profile(id: string, name: string, language: "sv" | "en", shortcut: string): HotkeyProfile {
  return { id, name, shortcut, language, push_to_talk_shortcut: shortcut, toggle_shortcut: null };
}

const blocked = (profileId: string) =>
  backendError(
    "dictionary_blocks_language_change",
    `Profile '${profileId}' has a personal dictionary; clear it before changing the profile language`,
  );

async function openSettingsTab() {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Settings" }));
  return user;
}

describe("a language change a personal dictionary blocks", () => {
  test("clears and previews the dictionary of the first profile when none is called default", async () => {
    // Profiles [work, other]: `set_language` changes `work`, the first one.
    let stored = { work: "merge = merch", other: "other = words" };
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () =>
          settingsValue({
            hotkey_profiles: [profile("work", "Arbete", "sv", "Ctrl+Alt+W"), profile("other", "Other", "en", "Ctrl+Alt+O")],
            profile_glossaries: { ...stored },
          }),
        set_language: () => {
          throw blocked("work");
        },
        set_profile_glossary: ({ profileId, source }) => {
          stored = { ...stored, [profileId]: source };
        },
      }),
    );
    const user = await openSettingsTab();

    await user.selectOptions(screen.getByRole("combobox", { name: "Default profile language" }), "English");

    // The preview is work's dictionary, not an absent "default".
    const preview = (await screen.findByLabelText("Saved default profile dictionary")) as HTMLTextAreaElement;
    expect(preview.value).toBe("merge = merch");
    expect(screen.queryByText(/cleared elsewhere/)).toBeNull();

    ipc.handle("set_language", () => []);
    await user.click(screen.getByRole("button", { name: "Clear default dictionary and switch to English" }));

    await waitFor(() => expect(ipc.callsTo("set_language")).toHaveLength(2));
    expect(ipc.callsTo("set_profile_glossary")).toEqual([{ profileId: "work", source: "", expectedSource: "merge = merch" }]);
    expect(stored.other).toBe("other = words");
  });

  test("keeps using the profile called default when it is not the first", async () => {
    let stored = { work: "work = words", default: "merge = merch" };
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () =>
          settingsValue({
            hotkey_profiles: [profile("work", "Arbete", "sv", "Ctrl+Alt+W"), profile("default", "Svenska", "sv", "Ctrl+Shift+Space")],
            profile_glossaries: { ...stored },
          }),
        set_language: () => {
          throw blocked("default");
        },
        set_profile_glossary: ({ profileId, source }) => {
          stored = { ...stored, [profileId]: source };
        },
      }),
    );
    const user = await openSettingsTab();

    await user.selectOptions(screen.getByRole("combobox", { name: "Default profile language" }), "English");

    const preview = (await screen.findByLabelText("Saved default profile dictionary")) as HTMLTextAreaElement;
    expect(preview.value).toBe("merge = merch");
    ipc.handle("set_language", () => []);
    await user.click(screen.getByRole("button", { name: "Clear default dictionary and switch to English" }));

    await waitFor(() => expect(ipc.callsTo("set_language")).toHaveLength(2));
    expect(ipc.callsTo("set_profile_glossary")).toEqual([{ profileId: "default", source: "", expectedSource: "merge = merch" }]);
    expect(stored.work).toBe("work = words");
  });
});
