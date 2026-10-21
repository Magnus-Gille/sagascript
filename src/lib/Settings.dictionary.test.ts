// Settings.svelte's personal dictionary editor, exercised through its rendered
// controls and typed IPC boundary (#361).
import settingsFixture from "../../contracts/transport/get_settings.json";
import { screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test, vi } from "vitest";
import type { Settings as SettingsValue } from "./api";
import { backendError } from "../test/ipc";
import { deferred } from "../test/deferred";
import { renderSettings, settingsBackend } from "../test/settings";

const defaultSettings = settingsFixture.default as SettingsValue;
const customisedSettings = settingsFixture.customised as SettingsValue;

afterEach(() => vi.unstubAllGlobals());

function saveDictionary(settings: SettingsValue, profileId: string, source: string): SettingsValue {
  return {
    ...settings,
    profile_glossaries: { ...settings.profile_glossaries, [profileId]: source },
  };
}

async function openDictionary(settings = defaultSettings) {
  let current = settings;
  const backend = settingsBackend({
    get_settings: () => current,
    set_profile_glossary: ({ profileId, source }) => {
      current = saveDictionary(current, profileId, source);
    },
  });
  const { ipc } = await renderSettings(backend);
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Settings" }));
  const textarea = (await screen.findByRole("textbox", { name: "Personal dictionary" })) as HTMLTextAreaElement;
  return { ipc, textarea, user };
}

function dictionaryScopePicker(): HTMLSelectElement {
  return screen.getByRole("combobox", { name: "" }) as HTMLSelectElement;
}

describe("personal dictionary editor", () => {
  test("editing is local until Save and the dirty status follows the saved baseline", async () => {
    const write = deferred<void>();
    let current = defaultSettings;
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => current,
        set_profile_glossary: ({ profileId, source }) =>
          write.promise.then(() => {
            current = saveDictionary(current, profileId, source);
          }),
      }),
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const textarea = (await screen.findByRole("textbox", { name: "Personal dictionary" })) as HTMLTextAreaElement;
    const save = screen.getByRole("button", { name: "Save changes" });
    const discard = screen.getByRole("button", { name: "Discard changes" });

    expect(textarea).toHaveValue("");
    expect(screen.queryByText("Unsaved changes")).toBeNull();
    expect(save).toBeDisabled();
    expect(discard).toBeDisabled();

    await user.type(textarea, "OpenAI = open eye");
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
    expect(save).toBeEnabled();
    expect(discard).toBeEnabled();
    await user.tab();
    expect(textarea).toHaveValue("OpenAI = open eye");
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);

    await user.clear(textarea);
    expect(screen.queryByText("Unsaved changes")).toBeNull();
    expect(save).toBeDisabled();
    await user.type(textarea, "OpenAI = open eye");
    const beforeSave = ipc.calls.length;
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(screen.getByRole("button", { name: "Saving…" })).toBeDisabled());
    expect(ipc.callsTo("set_profile_glossary")).toEqual([{ profileId: "default", source: "OpenAI = open eye", expectedSource: "" }]);
    expect(ipc.calls.slice(beforeSave).map((call) => call.command)).toEqual(["set_profile_glossary"]);

    write.resolve();
    await waitFor(() => expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled());
    expect(screen.queryByText("Unsaved changes")).toBeNull();
    expect(ipc.calls.slice(beforeSave, beforeSave + 2).map((call) => call.command)).toEqual(["set_profile_glossary", "get_settings"]);
    expect(current.profile_glossaries.default).toBe("OpenAI = open eye");
  });

  test("Save sends the active profile and its saved source, including an absent source as empty", async () => {
    const { ipc, textarea, user } = await openDictionary(customisedSettings);
    const scope = dictionaryScopePicker();

    expect(scope).toHaveValue("swedish");
    expect(textarea).toHaveValue("mergea = mördsa");
    await user.clear(textarea);
    await user.type(textarea, "mergea = mördsa\nOpenAI = open eye");
    let beforeSave = ipc.calls.length;
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(ipc.callsTo("set_profile_glossary")).toHaveLength(1));
    expect(ipc.callsTo("set_profile_glossary")).toEqual([
      { profileId: "swedish", source: "mergea = mördsa\nOpenAI = open eye", expectedSource: "mergea = mördsa" },
    ]);
    await waitFor(() =>
      expect(
        ipc.calls
          .slice(beforeSave)
          .map((call) => call.command)
          .slice(0, 2),
      ).toEqual(["set_profile_glossary", "get_settings"]),
    );

    await user.selectOptions(scope, "finnish");
    await waitFor(() => expect(textarea).toHaveValue(""));
    expect(scope).toHaveValue("finnish");
    await user.type(textarea, "OpenAI = open eye");
    beforeSave = ipc.calls.length;
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(ipc.callsTo("set_profile_glossary")).toHaveLength(2));
    expect(ipc.callsTo("set_profile_glossary").at(-1)).toEqual({
      profileId: "finnish",
      source: "OpenAI = open eye",
      expectedSource: "",
    });
    await waitFor(() =>
      expect(
        ipc.calls
          .slice(beforeSave)
          .map((call) => call.command)
          .slice(0, 2),
      ).toEqual(["set_profile_glossary", "get_settings"]),
    );
  });

  test("Discard restores the saved dictionary without writing", async () => {
    const { ipc, textarea, user } = await openDictionary(customisedSettings);

    await user.clear(textarea);
    await user.type(textarea, "OpenAI = open eye");
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Discard changes" }));

    expect(textarea).toHaveValue("mergea = mördsa");
    expect(screen.queryByText("Unsaved changes")).toBeNull();
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);
  });

  test("dirty profile navigation offers Save, Discard, and Stay", async () => {
    const { ipc, textarea, user } = await openDictionary(customisedSettings);
    const scope = dictionaryScopePicker();

    await user.clear(textarea);
    await user.type(textarea, "mergea = mördsa\nOpenAI = open eye");
    await user.selectOptions(scope, "finnish");
    const dialog = screen.getByRole("dialog", { name: "Unsaved dictionary changes" });
    expect(dialog).toBeInTheDocument();
    expect(scope).toHaveValue("swedish");
    expect(textarea).toHaveValue("mergea = mördsa\nOpenAI = open eye");

    await user.click(within(dialog).getByRole("button", { name: "Stay" }));
    expect(screen.queryByRole("dialog", { name: "Unsaved dictionary changes" })).toBeNull();
    expect(scope).toHaveValue("swedish");
    expect(textarea).toHaveValue("mergea = mördsa\nOpenAI = open eye");

    await user.selectOptions(scope, "finnish");
    const discardDialog = screen.getByRole("dialog", { name: "Unsaved dictionary changes" });
    await user.click(within(discardDialog).getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(scope).toHaveValue("finnish"));
    expect(textarea).toHaveValue("");
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);

    await user.type(textarea, "OpenAI = open eye");
    await user.selectOptions(scope, "swedish");
    const saveDialog = screen.getByRole("dialog", { name: "Unsaved dictionary changes" });
    expect(scope).toHaveValue("finnish");
    await user.click(within(saveDialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(scope).toHaveValue("swedish"));
    expect(textarea).toHaveValue("mergea = mördsa");
    expect(ipc.callsTo("set_profile_glossary")).toEqual([{ profileId: "finnish", source: "OpenAI = open eye", expectedSource: "" }]);
  });

  test.each(["Stay", "Discard", "Save"] as const)(
    "dictionary tab decision %s resolves without meeting review confirmation",
    async (decision) => {
      const confirm = vi.fn().mockReturnValue(true);
      vi.stubGlobal("confirm", confirm);
      expect(window.confirm).toBe(confirm);
      const { ipc, textarea, user } = await openDictionary(customisedSettings);

      await user.clear(textarea);
      await user.type(textarea, "OpenAI = open eye");
      await user.click(screen.getByRole("button", { name: "Dictate" }));
      const dialog = screen.getByRole("dialog", { name: "Unsaved dictionary changes" });
      expect(dialog).toBeInTheDocument();
      expect(within(dialog).getByRole("button", { name: "Save" })).toBeInTheDocument();
      expect(within(dialog).getByRole("button", { name: "Discard" })).toBeInTheDocument();
      expect(within(dialog).getByRole("button", { name: "Stay" })).toBeInTheDocument();
      expect(screen.queryByText("Leave meeting review and discard unapplied edits")).toBeNull();

      await user.click(within(dialog).getByRole("button", { name: decision }));
      expect(screen.queryByRole("dialog", { name: "Unsaved dictionary changes" })).toBeNull();
      expect(screen.queryByText("Leave meeting review and discard unapplied edits")).toBeNull();
      expect(confirm).not.toHaveBeenCalled();

      if (decision === "Stay") {
        expect(textarea).toHaveValue("OpenAI = open eye");
        expect(ipc.callsTo("set_profile_glossary")).toEqual([]);
      } else {
        await screen.findByRole("button", { name: "Start recording" });
        await user.click(screen.getByRole("button", { name: "Settings" }));
        const restoredTextarea = (await screen.findByRole("textbox", { name: "Personal dictionary" })) as HTMLTextAreaElement;
        expect(screen.queryByRole("dialog", { name: "Unsaved dictionary changes" })).toBeNull();
        expect(restoredTextarea).toHaveValue(decision === "Save" ? "OpenAI = open eye" : "mergea = mördsa");
        expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
        expect(screen.getByRole("button", { name: "Discard changes" })).toBeDisabled();
        expect(confirm).not.toHaveBeenCalled();
        if (decision === "Save") {
          expect(ipc.callsTo("set_profile_glossary")).toEqual([
            { profileId: "swedish", source: "OpenAI = open eye", expectedSource: "mergea = mördsa" },
          ]);
        } else {
          expect(ipc.callsTo("set_profile_glossary")).toEqual([]);
        }
      }
    },
  );

  test.each([
    {
      name: "a write failure",
      code: "busy",
      message: "The dictionary write was refused.",
      conflict: false,
    },
    {
      name: "a dictionary conflict code",
      code: "dictionary_changed_elsewhere",
      message: "Dictionary changed elsewhere: the saved source no longer matches the editor baseline",
      conflict: true,
    },
    {
      name: "conflict-like message text without the conflict code",
      code: "busy",
      message: "Dictionary changed elsewhere: this message alone is not a conflict",
      conflict: false,
    },
  ])("$name preserves the draft and blocks navigation", async ({ code, message, conflict }) => {
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => defaultSettings,
        set_profile_glossary: () => {
          throw backendError(code, message);
        },
      }),
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const textarea = (await screen.findByRole("textbox", { name: "Personal dictionary" })) as HTMLTextAreaElement;
    await user.type(textarea, "draft that must survive");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(message);
    expect(textarea).toHaveValue("draft that must survive");
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
    if (conflict) {
      expect(screen.getByText(/This dictionary changed elsewhere\. Your draft is preserved/)).toBeInTheDocument();
    } else {
      expect(screen.queryByText(/This dictionary changed elsewhere\. Your draft is preserved/)).toBeNull();
    }

    await user.click(screen.getByRole("button", { name: "Dictate" }));
    const dialog = screen.getByRole("dialog", { name: "Unsaved dictionary changes" });
    expect(dialog).toBeInTheDocument();
    expect(textarea).toHaveValue("draft that must survive");
    expect(screen.queryByRole("button", { name: "Start recording" })).toBeNull();
    expect(ipc.callsTo("set_profile_glossary")).toHaveLength(1);
  });

  test("a language change in flight blocks dictionary actions and tab navigation", async () => {
    const languageChange = deferred<string[]>();
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => defaultSettings,
        set_language: () => languageChange.promise,
      }),
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const textarea = (await screen.findByRole("textbox", { name: "Personal dictionary" })) as HTMLTextAreaElement;
    await user.type(textarea, "OpenAI = open eye");
    const save = screen.getByRole("button", { name: "Save changes" });

    await user.selectOptions(screen.getByRole("combobox", { name: "Default profile language" }), "sv");
    await waitFor(() => expect(ipc.callsTo("set_language")).toEqual([{ language: "sv" }]));
    expect(save).toBeDisabled();
    expect(screen.getByRole("button", { name: "Dictate" })).toBeDisabled();
    await user.click(save);
    await user.click(screen.getByRole("button", { name: "Dictate" }));
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);
    expect(screen.queryByRole("dialog", { name: "Unsaved dictionary changes" })).toBeNull();
    expect(textarea).toHaveValue("OpenAI = open eye");

    languageChange.resolve([]);
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "Dictate" }));
    expect(screen.getByRole("dialog", { name: "Unsaved dictionary changes" })).toBeInTheDocument();
  });
});
