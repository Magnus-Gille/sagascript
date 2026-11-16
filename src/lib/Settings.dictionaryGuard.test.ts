// The Settings window's tab router while the dictionary has unsaved changes
// (SAGA-379): tray navigation, a tab click, a dictation error and dropped files
// all ask Save, Discard or Stay before the tab changes; nothing asks when clean.
import { emit } from "@tauri-apps/api/event";
import { screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import settingsFixture from "../../contracts/transport/get_settings.json";
import type { Settings as SettingsValue } from "./api";
import { backendError } from "../test/ipc";
import { deferred, settle } from "../test/deferred";
import { renderSettings, settingsBackend } from "../test/settings";

const customised = settingsFixture.customised as SettingsValue;
const SAVED = "mergea = mördsa";
const TYPED = "mergea = mördsa\nOpenAI = open eye";

async function openDictionary(extra: Parameters<typeof settingsBackend>[0] = {}) {
  let current = customised;
  const { ipc } = await renderSettings(
    settingsBackend({
      get_settings: () => current,
      set_profile_glossary: ({ profileId, source }) => {
        current = { ...current, profile_glossaries: { ...current.profile_glossaries, [profileId]: source } };
      },
      ...extra,
    }),
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Settings" }));
  const textarea = (await screen.findByRole("textbox", { name: "Personal dictionary" })) as HTMLTextAreaElement;
  return { ipc, user, textarea };
}

async function typeDraft(user: ReturnType<typeof userEvent.setup>, textarea: HTMLTextAreaElement) {
  await user.type(textarea, "\nOpenAI = open eye");
  expect(textarea).toHaveValue(TYPED);
}

const question = () => screen.queryByRole("dialog", { name: "Unsaved dictionary changes" });
const dictateTab = () => screen.queryByRole("button", { name: "Start recording" });
const resultTabs = () => screen.queryByRole("tablist", { name: "Transcription results" });
const drop = (paths: string[]) => emit("tauri://drag-drop", { paths, position: { x: 0, y: 0 } });

describe("the tray's navigate_tab event", () => {
  test("asks instead of changing the tab, Stay keeps the tab and Discard lands on Dictate", async () => {
    const { ipc, user, textarea } = await openDictionary();
    await typeDraft(user, textarea);

    ipc.emit("navigate_tab", "dictate");
    const dialog = await screen.findByRole("dialog", { name: "Unsaved dictionary changes" });
    expect(within(dialog).getByText("Save changes before leaving Settings?")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Personal dictionary" })).toBeInTheDocument();
    expect(dictateTab()).toBeNull();

    await user.click(within(dialog).getByRole("button", { name: "Stay" }));
    expect(question()).toBeNull();
    expect(textarea).toHaveValue(TYPED);
    expect(dictateTab()).toBeNull();

    ipc.emit("navigate_tab", "dictate");
    const again = await screen.findByRole("dialog", { name: "Unsaved dictionary changes" });
    await user.click(within(again).getByRole("button", { name: "Discard" }));
    expect(await screen.findByRole("button", { name: "Start recording" })).toBeInTheDocument();
    expect(question()).toBeNull();
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);
  });

  test("to the tab that is already showing asks nothing", async () => {
    const { ipc, user, textarea } = await openDictionary();
    await typeDraft(user, textarea);

    ipc.emit("navigate_tab", "settings");
    await settle();
    expect(question()).toBeNull();
    expect(textarea).toHaveValue(TYPED);
  });

  test("changes the tab at once when the dictionary is clean", async () => {
    const { ipc } = await openDictionary();

    ipc.emit("navigate_tab", "dictate");
    expect(await screen.findByRole("button", { name: "Start recording" })).toBeInTheDocument();
    expect(question()).toBeNull();
  });
});

describe("a tab button", () => {
  test("is held until Save wrote the dictionary, then the tab opens", async () => {
    const { ipc, user, textarea } = await openDictionary();
    await typeDraft(user, textarea);

    await user.click(screen.getByRole("button", { name: "Transcribe" }));
    const dialog = await screen.findByRole("dialog", { name: "Unsaved dictionary changes" });
    expect(screen.queryByRole("button", { name: "Open Files..." })).toBeNull();

    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("button", { name: "Open Files..." })).toBeInTheDocument();
    expect(ipc.callsTo("set_profile_glossary")).toEqual([{ profileId: "swedish", source: TYPED, expectedSource: SAVED }]);
    expect(question()).toBeNull();
  });
});

describe("a dictation error", () => {
  test("asks before it takes the window to the Dictate tab, and the draft stays", async () => {
    const { ipc, user, textarea } = await openDictionary();
    await typeDraft(user, textarea);

    ipc.emit("error", backendError("no_audio_captured", "No audio was captured."));

    await screen.findByRole("dialog", { name: "Unsaved dictionary changes" });
    expect(dictateTab()).toBeNull();
    expect(screen.getByRole("textbox", { name: "Personal dictionary" })).toHaveValue(TYPED);
  });
});

describe("dropped files", () => {
  test("ask first, queue nothing, and still nothing after Stay", async () => {
    const { user, textarea } = await openDictionary({ set_update_result_pending: () => undefined });
    await typeDraft(user, textarea);

    await drop(["/audio/one.wav"]);
    const dialog = await screen.findByRole("dialog", { name: "Unsaved dictionary changes" });
    expect(resultTabs()).toBeNull();

    await user.click(within(dialog).getByRole("button", { name: "Stay" }));
    await settle();
    expect(resultTabs()).toBeNull();
    expect(textarea).toHaveValue(TYPED);
  });

  test("Discard then opens the Transcribe tab with the dropped file queued", async () => {
    const result = deferred<string>();
    const { ipc, user, textarea } = await openDictionary({
      set_update_result_pending: () => undefined,
      transcribe_file: () => result.promise,
    });
    await typeDraft(user, textarea);

    await drop(["/audio/one.wav"]);
    const dialog = await screen.findByRole("dialog", { name: "Unsaved dictionary changes" });
    await user.click(within(dialog).getByRole("button", { name: "Discard" }));

    expect(await screen.findByRole("tablist", { name: "Transcription results" })).toBeInTheDocument();
    await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(1));
    expect(ipc.callsTo("transcribe_file")[0].filePath).toBe("/audio/one.wav");
    expect(screen.getAllByRole("tab").some((tab) => tab.title === "/audio/one.wav")).toBe(true);

    result.resolve("Dropped file result");
    expect(await screen.findByDisplayValue("Dropped file result")).toBeInTheDocument();
  });
});
