// Settings.svelte rendered against the typed IPC mock (#361): what the window
// shows and which commands its controls invoke. Replaces the source-text
// checks of scripts/test-settings-layout.mjs; the mapping is in
// docs/testing/frontend.md. Update recovery: Settings.recovery.test.ts.
import { screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import type { HotkeyProfile } from "./api";
import {
  engineStatus,
  kbWhisperMedium,
  pianissimo,
  renderSettings,
  settingsBackend,
  settingsValue,
  swedishProfile,
  whisperModel,
} from "../test/settings";

const englishProfile: HotkeyProfile = {
  id: "english",
  name: "English",
  shortcut: "Ctrl+Alt+E",
  language: "en",
  push_to_talk_shortcut: null,
  toggle_shortcut: "Ctrl+Alt+E",
};

/** True when `first` comes before `second` in document order. */
function precedes(first: Element, second: Element): boolean {
  return Boolean(first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING);
}

function optionLabels(select: HTMLElement): string[] {
  return within(select)
    .getAllByRole("option")
    .map((option) => option.textContent?.trim() ?? "");
}

async function openSettingsTab() {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Settings" }));
  await user.click(await screen.findByText("Advanced"));
  return user;
}

describe("window header and navigation", () => {
  test("the build identity sits in the one header, above the Dictate, Transcribe and Settings tabs in that order", async () => {
    await renderSettings();

    const [header, ...otherHeaders] = screen.getAllByRole("banner");
    expect(otherHeaders).toEqual([]);
    expect(within(header).getByLabelText("Build information")).toHaveTextContent(
      "Version 1.4.4 · Build a69d5d1 · 2026-10-13 · Engine 0.9.0 (0123456)",
    );

    const tabs = ["Dictate", "Transcribe", "Settings"].map((name) => screen.getByRole("button", { name }));
    expect(new Set(tabs.map((tab) => tab.parentElement)).size).toBe(1);
    expect(precedes(header, tabs[0])).toBe(true);
    expect(precedes(tabs[0], tabs[1]) && precedes(tabs[1], tabs[2])).toBe(true);
  });

  test("the header shows the engine host identity only when the host reports a version", async () => {
    await renderSettings(settingsBackend({ engine_status: () => ({ ...engineStatus, host_version: null, host_git_sha: null }) }));
    expect(screen.getByLabelText("Build information")).toHaveTextContent(/^Version 1\.4\.4 · Build a69d5d1 · 2026-10-13$/);
  });
});

describe("Dictate: profiles and recording shortcuts", () => {
  test("every profile offers both recording modes, each with its own shortcut and helper", async () => {
    const { ipc } = await renderSettings(
      settingsBackend({ get_settings: () => settingsValue({ hotkey_profiles: [swedishProfile, englishProfile] }) }),
    );

    expect(screen.getByRole("button", { name: "+ Add profile" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "+ Add language" })).toBeNull();
    for (const profile of ["Svenska", "English"]) {
      expect(screen.getByRole("button", { name: `${profile} Hold to record shortcut` })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: `${profile} Press to start/stop shortcut` })).toBeInTheDocument();
    }
    expect(screen.getByRole("button", { name: "Svenska Hold to record shortcut" })).toHaveTextContent("Ctrl + Shift + Space");
    expect(screen.getByRole("button", { name: "Svenska Press to start/stop shortcut" })).toHaveTextContent("Not set");
    expect(screen.getByRole("button", { name: "English Press to start/stop shortcut" })).toHaveTextContent("Ctrl + Alt + E");
    expect(screen.getAllByText("Hold to record")).toHaveLength(2);
    expect(screen.getAllByText("Press to start/stop")).toHaveLength(2);
    expect(screen.getAllByText("Hold the shortcut while speaking. Release to stop.")).toHaveLength(2);
    expect(screen.getAllByText("Press the shortcut to start recording. Press it again to stop.")).toHaveLength(2);
    expect(screen.getByText(/^Each shortcut above is independent and either or both may be configured\./)).toBeInTheDocument();
    expect(screen.queryByText("Push to talk")).toBeNull();
    expect(screen.queryByText("Toggle")).toBeNull();
    expect(ipc.unmocked).toEqual([]);
  });

  test("each profile has its own speech model picker, filled from the backend lineup for its language", async () => {
    const englishModels = [whisperModel("base.en", "Base (English)"), whisperModel("small.en", "Small (English)")];
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => settingsValue({ hotkey_profiles: [swedishProfile, englishProfile] }),
        get_file_model_options: ({ language }) => (language === "en" ? englishModels : [pianissimo, kbWhisperMedium]),
        get_effective_model_info: ({ language }) => (language === "en" ? englishModels[0] : pianissimo),
        set_profile_model: () => undefined,
      }),
    );

    const pickers = screen.getAllByRole("combobox", { name: /^Model for / });
    expect(pickers.map((picker) => picker.getAttribute("aria-label"))).toEqual(["Model for Svenska", "Model for English"]);
    expect(screen.getByText("Speech model for Svenska")).toContainElement(pickers[0]);
    expect(screen.getByText("Speech model for English")).toContainElement(pickers[1]);
    await waitFor(() => expect(optionLabels(pickers[0])).toEqual(["Recommended for Swedish", "Pianissimo", "KB-Whisper Medium"]));
    expect(optionLabels(pickers[1])).toEqual(["Recommended for English", "Base (English)", "Small (English)"]);
    expect(ipc.callsTo("get_profile_model_info")).toEqual(expect.arrayContaining([{ profileId: "default" }, { profileId: "english" }]));
    expect(ipc.callsTo("get_file_model_options")).toEqual(
      expect.arrayContaining([
        { language: "sv", include: null },
        { language: "en", include: null },
      ]),
    );
    expect(screen.queryByText(/Pianissimo is experimental and applies to every Swedish dictation shortcut/)).toBeNull();

    await userEvent.setup().selectOptions(pickers[1], "small.en");
    await waitFor(() => expect(ipc.callsTo("set_profile_model")).toEqual([{ profileId: "english", modelId: "small.en" }]));
  });
});

describe("Swedish model lineup", () => {
  const kbWhisperSmall = whisperModel("kb-whisper-small", "KB-Whisper Small", { size_mb: 466 });

  test("a profile on a retired model is offered the recommended one, downloaded before the switch", async () => {
    const notDownloaded = { ...pianissimo, downloaded: false };
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => settingsValue({ profile_models: { default: "kb-whisper-small" } }),
        get_profile_model_info: () => kbWhisperSmall,
        // The backend still lists the profile's current model so the picker can show it.
        get_file_model_options: ({ include }) => (include === "kb-whisper-small" ? [notDownloaded, kbWhisperMedium, kbWhisperSmall] : []),
        get_effective_model_info: () => notDownloaded,
        download_pianissimo_model: () => undefined,
        set_profile_model: () => undefined,
      }),
    );

    expect(await screen.findByText("Pianissimo is faster and more accurate for Swedish.")).toBeInTheDocument();
    const picker = screen.getByRole("combobox", { name: "Model for Svenska" });
    // Only what the backend lineup returns, never a hardcoded KB-Whisper list.
    expect(optionLabels(picker)).toEqual(["Recommended for Swedish", "Pianissimo", "KB-Whisper Medium", "KB-Whisper Small"]);
    expect(ipc.callsTo("get_file_model_options")).toContainEqual({ language: "sv", include: "kb-whisper-small" });

    const before = ipc.calls.length;
    await userEvent.setup().click(screen.getByRole("button", { name: "Switch to Pianissimo (download 640 MB)" }));
    await waitFor(() => expect(ipc.callsTo("set_profile_model")).toEqual([{ profileId: "default", modelId: "pianissimo-sv" }]));
    const switched = ipc.calls.slice(before).map((call) => call.command);
    expect(switched.slice(0, 3)).toEqual(["download_pianissimo_model", "set_profile_model", "get_settings"]);
    expect(ipc.callsTo("download_model")).toEqual([]);
  });

  test("a downloaded recommendation switches without a download, and a current model gets no suggestion", async () => {
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => settingsValue({ hotkey_profiles: [swedishProfile, { ...englishProfile, language: "sv", name: "Möten" }] }),
        get_profile_model_info: ({ profileId }) => (profileId === "default" ? kbWhisperSmall : kbWhisperMedium),
        set_profile_model: () => undefined,
      }),
    );

    const switchButtons = await screen.findAllByRole("button", { name: /^Switch to / });
    expect(switchButtons.map((button) => button.textContent?.trim())).toEqual(["Switch to Pianissimo"]);
    await userEvent.setup().click(switchButtons[0]);
    await waitFor(() => expect(ipc.callsTo("set_profile_model")).toEqual([{ profileId: "default", modelId: "pianissimo-sv" }]));
    expect(ipc.callsTo("download_pianissimo_model")).toEqual([]);
  });

  test("the file picker's Auto entry names the model Auto resolves to", async () => {
    const { ipc } = await renderSettings(
      settingsBackend({
        get_auto_file_model_info: () => kbWhisperMedium,
        get_effective_model_info: () => ({ ...pianissimo, downloaded: false }),
      }),
    );
    await userEvent.setup().click(screen.getByRole("button", { name: "Transcribe" }));
    const picker = screen.getByRole("combobox", { name: "File transcription model" });
    await waitFor(() => expect(optionLabels(picker)[0]).toBe("Auto — KB-Whisper Medium (downloaded)"));
    expect(optionLabels(picker).slice(1)).toEqual(["Pianissimo · 640 MB · ready", "KB-Whisper Medium · 1530 MB · ready"]);
    expect(ipc.callsTo("get_auto_file_model_info")).toContainEqual({ language: "sv" });
  });

  test("Auto shows (recommended) when it already uses the recommended model", async () => {
    await renderSettings();
    await userEvent.setup().click(screen.getByRole("button", { name: "Transcribe" }));
    const picker = screen.getByRole("combobox", { name: "File transcription model" });
    await waitFor(() => expect(optionLabels(picker)[0]).toBe("Auto — Pianissimo (recommended)"));
  });
});

describe("Pianissimo engine controls", () => {
  test("Advanced offers when to prepare and when to unload Pianissimo, and saves each choice", async () => {
    const { ipc } = await renderSettings(
      settingsBackend({ set_engine_prewarm: () => undefined, set_engine_idle_unload_minutes: () => undefined }),
    );
    const user = await openSettingsTab();

    const prewarm = screen.getByRole("combobox", { name: "Prepare Pianissimo when" });
    expect(optionLabels(prewarm)).toEqual(["When Sagascript starts (recommended)", "When I press the dictation key", "Only when needed"]);
    expect(prewarm).toHaveValue("on_key_down");
    const idle = screen.getByRole("combobox", { name: "Unload after idle" });
    expect(optionLabels(idle)).toEqual(["5 minutes", "10 minutes", "30 minutes", "60 minutes", "Never"]);

    await user.selectOptions(prewarm, "on_app_start");
    await user.selectOptions(idle, "Never");
    await waitFor(() => {
      expect(ipc.callsTo("set_engine_prewarm")).toEqual([{ mode: "on_app_start" }]);
      expect(ipc.callsTo("set_engine_idle_unload_minutes")).toEqual([{ minutes: 0 }]);
    });
    expect(ipc.callsTo("engine_status")).toHaveLength(1);
  });

  test("the engine controls stay hidden when Pianissimo is unsupported", async () => {
    await renderSettings(settingsBackend({ engine_status: () => ({ ...engineStatus, supported: false }) }));
    await openSettingsTab();
    expect(screen.queryByRole("combobox", { name: "Prepare Pianissimo when" })).toBeNull();
    expect(screen.queryByRole("combobox", { name: "Unload after idle" })).toBeNull();
  });

  test("choosing Pianissimo for files states its platform requirement and download size", async () => {
    await renderSettings(settingsBackend({ get_settings: () => settingsValue({ file_transcription_model: "pianissimo-sv" }) }));
    await userEvent.setup().click(screen.getByRole("button", { name: "Transcribe" }));
    expect(
      await screen.findByText(
        "Requires macOS 14+ on Apple Silicon or Windows on ARM (Snapdragon). Runs locally (Neural Engine on Mac, CPU on Windows); one-time download of about 640 MB.",
      ),
    ).toBeInTheDocument();
  });

  test("no tab shows outdated Pianissimo size, model or macOS version", async () => {
    await renderSettings(settingsBackend({ get_settings: () => settingsValue({ file_transcription_model: "pianissimo-sv" }) }));
    const seen: string[] = [document.body.textContent ?? ""];
    const user = await openSettingsTab();
    seen.push(document.body.textContent ?? "");
    await user.click(screen.getByRole("button", { name: "Transcribe" }));
    seen.push(document.body.textContent ?? "");
    for (const text of seen) expect(text).not.toMatch(/714|Pianissimo Q8|macOS 13/);
  });
});
