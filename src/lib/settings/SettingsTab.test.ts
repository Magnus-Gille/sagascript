// SettingsTab on its own (SAGA-379): the default language and its dictionary
// recovery, the overlay switch, the tab guard of the personal dictionary and
// the lock while the settings are unconfirmed. The same flows through the whole
// window: Settings.languageRecovery.test.ts, Settings.dictionaryGuard.test.ts
// and Settings.saving.test.ts.
import { screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import type { Settings as SettingsValue } from "../api";
import { deferred } from "../../test/deferred";
import { backendError } from "../../test/ipc";
import { renderPanel } from "../../test/settingsPanel";
import { settingsBackend, settingsValue } from "../../test/settings";
import SettingsTab from "./SettingsTab.svelte";

const languageSelect = () => screen.getByRole("combobox", { name: "Default profile language" });
const overlaySwitch = () => screen.getByRole("switch", { name: "Show recording overlay" });
const dictionary = () => screen.getByRole("textbox", { name: "Personal dictionary" });
const question = () => screen.queryByRole("dialog", { name: "Unsaved dictionary changes" });

/** A settings file in memory that `set_language` and `set_profile_glossary` change. */
function settingsFile(initial: Partial<SettingsValue> = {}) {
  const file = { current: settingsValue(initial) };
  return {
    file,
    handlers: settingsBackend({
      get_settings: () => structuredClone(file.current),
      set_language: ({ language }) => {
        file.current = { ...file.current, language };
        return [];
      },
      set_profile_glossary: ({ profileId, source }) => {
        file.current = { ...file.current, profile_glossaries: { ...file.current.profile_glossaries, [profileId]: source } };
      },
      set_show_overlay: ({ enabled }) => {
        file.current = { ...file.current, show_overlay: enabled };
      },
    }),
  };
}

describe("the Settings tab being shown", () => {
  test("shows the language, the overlay switch, the dictionary and a closed Advanced section while active", async () => {
    await renderPanel(SettingsTab, settingsFile().handlers);

    expect(languageSelect()).toHaveDisplayValue("Swedish");
    expect(overlaySwitch()).toHaveAttribute("aria-checked", "true");
    expect(dictionary()).toBeInTheDocument();
    expect(screen.getByText("Advanced").closest("details")).not.toHaveAttribute("open");
  });

  test("shows none of its controls while hidden, and a dictionary draft typed before is still there when shown again", async () => {
    const { setActive } = await renderPanel(SettingsTab, settingsFile().handlers);
    const user = userEvent.setup();
    await user.type(dictionary(), "merge = merch");

    await setActive(false);
    expect(screen.queryByRole("combobox", { name: "Default profile language" })).toBeNull();
    expect(screen.queryByRole("switch", { name: "Show recording overlay" })).toBeNull();
    expect(screen.queryByRole("textbox", { name: "Personal dictionary" })).toBeNull();
    expect(screen.queryByText("Advanced")).toBeNull();

    await setActive(true);
    expect(dictionary()).toHaveValue("merge = merch");
    expect(screen.getByRole("status")).toHaveTextContent("Unsaved changes");
  });
});

describe("choosing the default language", () => {
  test("sends set_language, disables the select and holds the tabs while the command is in flight", async () => {
    const file = settingsFile();
    const gate = deferred<string[]>();
    const { ipc, win } = await renderPanel(SettingsTab, {
      ...file.handlers,
      set_language: async ({ language }) => {
        const notices = await gate.promise;
        file.file.current = { ...file.file.current, language };
        return notices;
      },
    });
    const user = userEvent.setup();

    await user.selectOptions(languageSelect(), "English");

    await waitFor(() => expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]));
    await waitFor(() => expect(languageSelect()).toBeDisabled());
    expect(win.router.held).toBe(true);

    gate.resolve([]);
    await waitFor(() => expect(languageSelect()).toBeEnabled());
    expect(win.router.held).toBe(false);
    expect(languageSelect()).toHaveDisplayValue("English");
  });

  test("shows the saved language again and reports the message when the backend rejects the change", async () => {
    const { win } = await renderPanel(SettingsTab, {
      ...settingsFile().handlers,
      set_language: () => {
        throw backendError("settings_storage_failed", "disk full");
      },
    });
    const user = userEvent.setup();

    await user.selectOptions(languageSelect(), "English");

    await waitFor(() => expect(win.errors.of("settings").message).toBe("disk full"));
    expect(win.errors.of("dictate").message).toBe("");
    await waitFor(() => expect(languageSelect()).toBeEnabled());
    expect(languageSelect()).toHaveDisplayValue("Swedish");
    expect(win.router.held).toBe(false);
  });
});

describe("a language change the default dictionary blocks", () => {
  /** The backend refuses the change while the default profile has a dictionary, as the real one does. */
  function blockingBackend() {
    const file = settingsFile({ profile_glossaries: { default: "merge = merch" } });
    return {
      file,
      handlers: {
        ...file.handlers,
        set_language: ({ language }: { language: SettingsValue["language"] }) => {
          if (file.file.current.profile_glossaries.default?.trim()) {
            throw backendError("dictionary_blocks_language_change", "Profile 'default' has a personal dictionary");
          }
          file.file.current = { ...file.file.current, language };
          return [];
        },
      },
    };
  }

  test("offers the saved dictionary read-only with the focus on the heading, and Cancel returns the focus to the select", async () => {
    const { ipc } = await renderPanel(SettingsTab, blockingBackend().handlers);
    const user = userEvent.setup();

    await user.selectOptions(languageSelect(), "English");

    const heading = await screen.findByRole("heading", { name: "Switch to English?" });
    await waitFor(() => expect(heading).toHaveFocus());
    const preview = screen.getByLabelText("Saved default profile dictionary");
    expect(preview).toHaveValue("merge = merch");
    expect(preview).toHaveAttribute("readonly");
    expect(languageSelect()).toHaveDisplayValue("Swedish");

    await user.click(screen.getByRole("button", { name: "Cancel" }));

    expect(screen.queryByRole("heading", { name: "Switch to English?" })).toBeNull();
    await waitFor(() => expect(languageSelect()).toHaveFocus());
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);
    expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]);
  });

  test("clears the default dictionary against the previewed text, then sets the language", async () => {
    const backend = blockingBackend();
    const { ipc } = await renderPanel(SettingsTab, backend.handlers);
    const user = userEvent.setup();
    await user.selectOptions(languageSelect(), "English");
    await screen.findByRole("heading", { name: "Switch to English?" });

    await user.click(screen.getByRole("button", { name: "Clear default dictionary and switch to English" }));

    await waitFor(() => expect(languageSelect()).toHaveDisplayValue("English"));
    expect(ipc.callsTo("set_profile_glossary")).toEqual([{ profileId: "default", source: "", expectedSource: "merge = merch" }]);
    expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }, { language: "en" }]);
    expect(
      ipc.calls.filter((call) => call.command === "set_profile_glossary" || call.command === "set_language").map((c) => c.command),
    ).toEqual(["set_language", "set_profile_glossary", "set_language"]);
    expect(screen.queryByRole("heading", { name: "Switch to English?" })).toBeNull();
    expect(backend.file.file.current.language).toBe("en");
  });
});

describe("the recording overlay switch", () => {
  test("sends the opposite of the saved value and shows the new one", async () => {
    const { ipc } = await renderPanel(SettingsTab, settingsFile({ show_overlay: true }).handlers);
    const user = userEvent.setup();

    await user.click(overlaySwitch());
    await waitFor(() => expect(overlaySwitch()).toHaveAttribute("aria-checked", "false"));
    expect(ipc.callsTo("set_show_overlay")).toEqual([{ enabled: false }]);

    await user.click(overlaySwitch());
    await waitFor(() => expect(overlaySwitch()).toHaveAttribute("aria-checked", "true"));
    expect(ipc.callsTo("set_show_overlay")).toEqual([{ enabled: false }, { enabled: true }]);
  });
});

describe("the tab's error", () => {
  test("a rejected overlay change is reported on the Settings tab and the switch keeps the saved value", async () => {
    const { win } = await renderPanel(SettingsTab, {
      ...settingsFile({ show_overlay: true }).handlers,
      set_show_overlay: () => {
        throw backendError("settings_storage_failed", "disk full");
      },
    });

    await userEvent.setup().click(overlaySwitch());

    await waitFor(() => expect(win.errors.of("settings").message).toBe("disk full"));
    expect(win.errors.of("dictate").message).toBe("");
    expect(overlaySwitch()).toHaveAttribute("aria-checked", "true");
  });

  test("a rejected dictionary save is reported on the Settings tab", async () => {
    const { ipc, win } = await renderPanel(SettingsTab, {
      ...settingsFile().handlers,
      set_profile_glossary: () => {
        throw backendError("busy", "The dictionary write was refused.");
      },
    });
    const user = userEvent.setup();
    await user.type(dictionary(), "merge = merch");

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(win.errors.of("settings").message).toBe("The dictionary write was refused."));
    expect(win.errors.of("dictate").message).toBe("");
    expect(ipc.callsTo("set_profile_glossary")).toHaveLength(1);
    expect(dictionary()).toHaveValue("merge = merch");
  });

  test("a saved change clears the tab's own error and leaves the Dictate tab's", async () => {
    const { ipc, win } = await renderPanel(SettingsTab, settingsFile({ show_overlay: true }).handlers);
    win.errors.of("settings").message = "An earlier failure on Settings";
    win.errors.of("dictate").message = "An earlier failure on Dictate";

    await userEvent.setup().click(overlaySwitch());

    await waitFor(() => expect(ipc.callsTo("set_show_overlay")).toEqual([{ enabled: false }]));
    await waitFor(() => expect(win.errors.of("settings").message).toBe(""));
    expect(win.errors.of("dictate").message).toBe("An earlier failure on Dictate");
  });
});

describe("the tab guard of the personal dictionary", () => {
  test("with an unsaved draft, a tab request asks first and Stay keeps the tab and the draft", async () => {
    const { win } = await renderPanel(SettingsTab, settingsFile().handlers);
    win.router.active = "settings";
    const user = userEvent.setup();
    await user.type(dictionary(), "merge = merch");

    win.router.request("dictate");

    const dialog = await screen.findByRole("dialog", { name: "Unsaved dictionary changes" });
    expect(win.router.active).toBe("settings");
    expect(win.router.asking).toBe(true);

    await user.click(within(dialog).getByRole("button", { name: "Stay" }));

    expect(question()).toBeNull();
    expect(win.router.asking).toBe(false);
    expect(win.router.active).toBe("settings");
    expect(dictionary()).toHaveValue("merge = merch");
  });

  test("with a clean dictionary, a tab request changes the tab at once", async () => {
    const { win } = await renderPanel(SettingsTab, settingsFile().handlers);
    win.router.active = "settings";

    win.router.request("dictate");

    expect(win.router.active).toBe("dictate");
    expect(win.router.asking).toBe(false);
    expect(question()).toBeNull();
  });
});

describe("while the settings are unconfirmed", () => {
  test("the language select and the overlay switch are disabled", async () => {
    const { win } = await renderPanel(SettingsTab, settingsFile().handlers);
    expect(languageSelect()).toBeEnabled();
    expect(overlaySwitch()).toBeEnabled();

    win.session.markUnconfirmed("settings read timed out", false);

    await waitFor(() => expect(languageSelect()).toBeDisabled());
    expect(overlaySwitch()).toBeDisabled();
  });
});
