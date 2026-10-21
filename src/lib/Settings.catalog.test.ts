import { act, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import { deferred } from "../test/deferred";
import { kbWhisperMedium, pianissimo, renderSettings, settingsBackend, settingsValue, whisperModel } from "../test/settings";

describe("Settings model catalog refresh", () => {
  test("a late file catalog response cannot replace the catalog for the newer language", async () => {
    const firstLanguage = deferred<typeof pianissimo>();
    const secondLanguage = deferred<typeof pianissimo>();
    let settings = settingsValue();
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => settings,
        set_language: ({ language }) => {
          settings = settingsValue({
            ...settings,
            language,
            hotkey_profiles: settings.hotkey_profiles.map((profile) => ({ ...profile, language })),
          });
          return [];
        },
        get_file_model_options: ({ language }) =>
          [pianissimo, kbWhisperMedium].map((model) => ({
            ...model,
            description: `Options for ${language}`,
          })),
        get_auto_file_model_info: ({ language }) => (language === "sv" ? firstLanguage.promise : secondLanguage.promise),
        get_effective_model_info: ({ language }) => (language === "en" ? kbWhisperMedium : pianissimo),
      }),
    );

    await waitFor(() => expect(ipc.callsTo("get_auto_file_model_info")).toEqual([{ language: "sv" }]));
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    await user.selectOptions(screen.getByLabelText("Default profile language"), "en");
    await waitFor(() => expect(ipc.callsTo("get_auto_file_model_info")).toContainEqual({ language: "en" }));

    const latestAutomatic = whisperModel("latest-auto", "Latest automatic model");
    secondLanguage.resolve(latestAutomatic);
    await user.click(screen.getByRole("button", { name: "Transcribe" }));
    expect(await screen.findByText(/Auto — Latest automatic model/)).toBeInTheDocument();

    await act(async () => {
      firstLanguage.resolve(pianissimo);
      // Drain the old Promise.all/controller continuation before act flushes
      // Svelte state and the stale-publication assertion runs.
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(screen.getByText(/Auto — Latest automatic model/)).toBeInTheDocument();
    expect(screen.queryByText(/Auto — Pianissimo/)).toBeNull();
  });
});
