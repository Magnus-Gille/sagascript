// Onboarding.svelte rendered against the typed IPC mock (#361). Replaces the
// Onboarding source-text checks of scripts/test-settings-layout.mjs; the
// mapping is in docs/testing/frontend.md.
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test, vi } from "vitest";
import Onboarding from "./Onboarding.svelte";
import { deferred } from "../test/deferred";
import { backendError, mockIpc, type CommandHandlers } from "../test/ipc";
import { kbWhisperMedium, pianissimo, settingsValue, whisperModel } from "../test/settings";

/** A first start on Windows: no permission steps (language, download, ready). */
function backend(overrides: CommandHandlers = {}): CommandHandlers {
  return {
    get_platform: () => "windows",
    microphone_status: () => "unsupported",
    check_accessibility_permission: () => true,
    get_settings: () => settingsValue({ language: "en", has_completed_onboarding: false }),
    set_language: () => [],
    get_effective_model_info: () => pianissimo,
    download_pianissimo_model: () => undefined,
    download_model: () => undefined,
    ...overrides,
  };
}

async function renderOnboarding(handlers: CommandHandlers) {
  const ipc = mockIpc(handlers);
  render(Onboarding, { oncomplete: vi.fn() });
  const user = userEvent.setup();
  // Continue is enabled once the platform is known.
  await waitFor(() => expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled());
  return { ipc, user };
}

async function chooseSwedish(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "SV Svenska" }));
  await user.click(screen.getByRole("button", { name: "Continue" }));
}

async function chooseFinnish(user: ReturnType<typeof userEvent.setup>) {
  const english = screen.getByRole("button", { name: "EN English" });
  const finnish = screen.getByRole("button", { name: "FI Suomi" });
  expect(english).toHaveAttribute("aria-pressed", "true");
  expect(finnish).toHaveAttribute("aria-pressed", "false");
  await user.click(finnish);
  expect(finnish).toHaveAttribute("aria-pressed", "true");
  expect(english).toHaveAttribute("aria-pressed", "false");
  await user.click(screen.getByRole("button", { name: "Continue" }));
}

describe("Finnish onboarding", () => {
  test("choosing Finnish saves fi and downloads the backend-recommended Base model", async () => {
    const catalogBase = whisperModel("base", "Catalog Base fixture", { size_mb: 188 });
    const { ipc, user } = await renderOnboarding(
      backend({
        get_effective_model_info: ({ language }) => {
          expect(language).toBe("fi");
          return catalogBase;
        },
      }),
    );

    await chooseFinnish(user);

    expect(await screen.findByText("Speech engine ready")).toBeInTheDocument();
    expect(screen.getByText(/Preparing the local speech engine \(188 MB\)/)).toBeInTheDocument();
    expect(ipc.callsTo("set_language")).toEqual([{ language: "fi" }]);
    expect(ipc.callsTo("get_effective_model_info")).toEqual([{ language: "fi" }]);
    expect(ipc.callsTo("download_model")).toEqual([{ whisperModel: "base" }]);
    expect(ipc.callsTo("download_pianissimo_model")).toEqual([]);
  });

  test("when the recommendation is unavailable, Finnish falls back to Base and shows 142 MB", async () => {
    const { ipc, user } = await renderOnboarding(
      backend({
        get_effective_model_info: () => {
          throw backendError("unavailable", "model lineup unavailable");
        },
      }),
    );

    await chooseFinnish(user);

    expect(await screen.findByText("Speech engine ready")).toBeInTheDocument();
    expect(screen.getByText(/Preparing the local speech engine \(142 MB\)/)).toBeInTheDocument();
    expect(ipc.callsTo("set_language")).toEqual([{ language: "fi" }]);
    expect(ipc.callsTo("get_effective_model_info")).toEqual([{ language: "fi" }]);
    expect(ipc.callsTo("download_model")).toEqual([{ whisperModel: "base" }]);
    expect(ipc.callsTo("download_pianissimo_model")).toEqual([]);
  });
});

describe("speech engine download", () => {
  test("Swedish downloads the model the backend recommends (Pianissimo where supported)", async () => {
    const { ipc, user } = await renderOnboarding(backend());
    await chooseSwedish(user);
    expect(await screen.findByText("Speech engine ready")).toBeInTheDocument();
    expect(ipc.callsTo("set_language")).toEqual([{ language: "sv" }]);
    expect(ipc.callsTo("get_effective_model_info")).toEqual([{ language: "sv" }]);
    expect(ipc.callsTo("download_pianissimo_model")).toEqual([{}]);
    expect(ipc.callsTo("download_model")).toEqual([]);
    expect(screen.getByText(/Preparing the local speech engine \(640 MB\)/)).toBeInTheDocument();
  });

  test("download events show the progress, and model-ready completes the step before the command settles", async () => {
    const download = deferred<void>();
    const { ipc, user } = await renderOnboarding(backend({ download_pianissimo_model: () => download.promise }));
    await chooseSwedish(user);
    await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toHaveLength(1));

    ipc.emit("model-download-progress", { progress: 42.4 });
    expect(await screen.findByText("42%")).toBeInTheDocument();

    ipc.emit("model-ready", null);
    expect(await screen.findByText("Speech engine ready")).toBeInTheDocument();
    download.resolve();
  });

  test("where the backend recommends KB-Whisper Medium, that model is downloaded", async () => {
    const { ipc, user } = await renderOnboarding(backend({ get_effective_model_info: () => kbWhisperMedium }));
    await chooseSwedish(user);
    expect(await screen.findByText("Speech engine ready")).toBeInTheDocument();
    expect(ipc.callsTo("download_model")).toEqual([{ whisperModel: "kb-whisper-medium" }]);
    expect(ipc.callsTo("download_pianissimo_model")).toEqual([]);
  });

  test("without a recommendation, Swedish falls back to KB-Whisper Medium, not a retired model", async () => {
    const { ipc, user } = await renderOnboarding(
      backend({
        get_effective_model_info: () => {
          throw backendError("unavailable", "model lineup unavailable");
        },
      }),
    );
    await chooseSwedish(user);
    expect(await screen.findByText("Speech engine ready")).toBeInTheDocument();
    expect(ipc.callsTo("download_model")).toEqual([{ whisperModel: "kb-whisper-medium" }]);
  });
});

test("the last step explains both recording modes and where to configure them", async () => {
  const { user } = await renderOnboarding(backend());
  await chooseSwedish(user);
  await user.click(await screen.findByRole("button", { name: "Continue" }));

  expect(await screen.findByRole("heading", { name: "You're All Set!" })).toBeInTheDocument();
  expect(screen.getByText("Configure separate shortcuts for these recording modes in Dictate.")).toBeInTheDocument();
  const modes = [
    ["Hold to record", "Hold the shortcut while speaking. Release to stop."],
    ["Press to start/stop", "Press the shortcut to start recording. Press it again to stop."],
  ];
  for (const [label, helper] of modes) {
    const labelElement = screen.getByText(label);
    // Each helper follows its own mode label.
    expect(labelElement.nextElementSibling).toHaveTextContent(helper);
  }
  expect(screen.queryByText(/Hold to record, release to transcribe/)).toBeNull();
});
