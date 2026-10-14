// What Settings asks the backend after a change (#362): a setting that no model
// depends on is a write and a read-back, not a fresh query of every profile's
// models. Language, shortcuts and models still re-read them.
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import type { CommandName } from "../test/ipc";
import { renderSettings, settingsBackend, settingsValue } from "../test/settings";

type User = ReturnType<typeof userEvent.setup>;

const backend = () =>
  settingsBackend({
    set_vad_enabled: () => undefined,
    set_show_overlay: () => undefined,
    set_auto_paste: () => [],
    set_beam_size: () => undefined,
    set_temperature_fallback: () => undefined,
    set_engine_prewarm: () => undefined,
    set_engine_idle_unload_minutes: () => undefined,
    set_language: () => [],
  });

/** Dictate is the tab Settings opens on; the controls below sit on Dictate or under Settings > Advanced. */
async function openAdvanced(user: User) {
  await user.click(screen.getByRole("button", { name: "Settings" }));
  await user.click(await screen.findByText("Advanced"));
}
const stayOnDictate = async () => undefined;

const toggle = (name: string) => (user: User) => user.click(screen.getByRole("switch", { name }));
const choose = (label: string, value: string) => (user: User) => user.selectOptions(screen.getByLabelText(label), value);

interface Change {
  label: string;
  command: CommandName;
  open: (user: User) => Promise<void>;
  act: (user: User) => Promise<unknown>;
}

const changes: Change[] = [
  { label: "Voice activity detection", command: "set_vad_enabled", open: openAdvanced, act: toggle("Voice activity detection") },
  { label: "Show recording overlay", command: "set_show_overlay", open: openAdvanced, act: toggle("Show recording overlay") },
  { label: "Auto-paste transcription", command: "set_auto_paste", open: stayOnDictate, act: toggle("Auto-paste transcription") },
  { label: "Decoding mode", command: "set_beam_size", open: openAdvanced, act: choose("Decoding mode", "0") },
  { label: "Temperature fallback", command: "set_temperature_fallback", open: openAdvanced, act: toggle("Temperature fallback") },
  { label: "Prepare Pianissimo when", command: "set_engine_prewarm", open: openAdvanced, act: choose("Prepare Pianissimo when", "off") },
  { label: "Unload after idle", command: "set_engine_idle_unload_minutes", open: openAdvanced, act: choose("Unload after idle", "30") },
];

describe("a setting that no model depends on", () => {
  test.each(changes)("$label: one write and one read-back, no model queries", async ({ command, open, act }) => {
    const { ipc } = await renderSettings(backend());
    const user = userEvent.setup();
    await open(user);
    const before = ipc.calls.length;

    await act(user);

    await waitFor(() => expect(ipc.calls.slice(before).map((call) => call.command)).toContain(command));
    // Leave time for anything else the change would trigger.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(ipc.calls.slice(before).map((call) => call.command)).toEqual([command, "get_settings"]);
  });
});

describe("a setting that changes which models apply", () => {
  test("a language change re-reads every profile's models", async () => {
    const { ipc } = await renderSettings(backend());
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const before = ipc.calls.length;

    await user.selectOptions(screen.getByLabelText("Default profile language"), "en");

    await waitFor(() => expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]));
    await waitFor(() => {
      const commands = ipc.calls.slice(before).map((call) => call.command);
      expect(commands.slice(0, 2)).toEqual(["set_language", "get_settings"]);
      expect(commands).toContain("get_profile_model_info");
      expect(commands).toContain("get_file_model_options");
      expect(commands).toContain("get_effective_model_info");
    });
  });

  test("the file model lineup is queried again when the language or the selection changes", async () => {
    let settings = settingsValue();
    const { ipc } = await renderSettings(settingsBackend({ get_settings: () => settings, set_language: () => [] }));
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const before = ipc.callsTo("get_auto_file_model_info").length;

    settings = settingsValue({ language: "en", hotkey_profiles: [{ ...settings.hotkey_profiles[0], language: "en" }] });
    await user.selectOptions(screen.getByLabelText("Default profile language"), "en");

    await waitFor(() => expect(ipc.callsTo("get_auto_file_model_info").length).toBeGreaterThan(before));
    expect(ipc.callsTo("get_auto_file_model_info").at(-1)).toEqual({ language: "en" });
  });
});
