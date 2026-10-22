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

describe("default onboarding language", () => {
  test("waits for the platform before it can submit a language", async () => {
    const platform = deferred<string>();
    const ipc = mockIpc(backend({ get_platform: () => platform.promise }));
    render(Onboarding, { oncomplete: vi.fn() });
    const user = userEvent.setup();

    const preparing = screen.getByRole("button", { name: "Preparing…" });
    expect(preparing).toBeDisabled();
    await user.click(preparing);
    expect(ipc.callsTo("set_language")).toEqual([]);

    platform.resolve("windows");
    await waitFor(() => expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]));
  });

  test("opens directly on language selection with the portable setup copy", async () => {
    const { ipc, user } = await renderOnboarding(backend());

    expect(screen.getByRole("heading", { name: "Set up Sagascript" })).toBeInTheDocument();
    expect(screen.getByText(/Choose your first profile's language/)).toBeInTheDocument();
    expect(screen.getByText(/recommended local model is selected automatically/)).toBeInTheDocument();
    expect(screen.queryByText(/Welcome to Sagascript/)).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /English|Svenska|Norsk|Suomi/ })).toHaveLength(4);
    expect(ipc.callsTo("set_language")).toEqual([]);
    await chooseSwedish(user);
    expect(await screen.findByRole("heading", { name: "Setting up speech engine" })).toBeInTheDocument();
    expect(screen.getByText(/Your recordings are processed on this device/)).toBeInTheDocument();
    expect(screen.queryByText(/mac(?:os)?/i)).not.toBeInTheDocument();
    expect(await screen.findByText("Speech engine ready")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(await screen.findByRole("heading", { name: "You're All Set!" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Microphone Access" })).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Accessibility Permission" })).not.toBeInTheDocument();
  });

  test("shows device-local copy and permission steps only on macOS", async () => {
    const { user } = await renderOnboarding(backend({ get_platform: () => "macos", set_auto_paste: () => undefined }));

    expect(screen.getByText(/recommended local model is selected automatically/)).toBeInTheDocument();
    await chooseSwedish(user);
    expect(await screen.findByRole("heading", { name: "Setting up speech engine" })).toBeInTheDocument();
    expect(screen.getByText(/Your recordings are processed on this device/)).toBeInTheDocument();
    expect(await screen.findByText("Speech engine ready")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(await screen.findByRole("heading", { name: "Microphone Access" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Skip" }));
    expect(await screen.findByRole("heading", { name: "Accessibility Permission" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(await screen.findByRole("heading", { name: "You're All Set!" })).toBeInTheDocument();
  });

  test("blocks repeated language submission while the save is pending", async () => {
    const save = deferred<string[]>();
    const { ipc, user } = await renderOnboarding(backend({ set_language: () => save.promise }));

    await user.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]));
    const saving = screen.getByRole("button", { name: "Saving…" });
    expect(saving).toBeDisabled();
    await user.click(saving);
    expect(screen.queryByRole("heading", { name: "Setting up speech engine" })).not.toBeInTheDocument();

    save.resolve([]);
    expect(await screen.findByRole("heading", { name: "Setting up speech engine" })).toBeInTheDocument();
    expect(ipc.callsTo("set_language")).toHaveLength(1);
  });

  test("keeps English selected when persisted language is auto", async () => {
    const settingsRead = deferred<ReturnType<typeof settingsValue>>();
    let settingsSettled = false;
    const { ipc, user } = await renderOnboarding(
      backend({
        get_settings: async () => {
          try {
            return await settingsRead.promise;
          } finally {
            settingsSettled = true;
          }
        },
      }),
    );

    await waitFor(() => expect(ipc.callsTo("get_settings")).toHaveLength(1));
    settingsRead.resolve(settingsValue({ language: "auto", has_completed_onboarding: false }));
    await waitFor(() => expect(settingsSettled).toBe(true));

    const english = screen.getByRole("button", { name: "EN English" });
    const finnish = screen.getByRole("button", { name: "FI Suomi" });
    expect(english).toHaveAttribute("aria-pressed", "true");
    expect(finnish).toHaveAttribute("aria-pressed", "false");

    await user.click(screen.getByRole("button", { name: "Continue" }));

    await waitFor(() => expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]));
  });

  test("keeps English selected when persisted settings cannot be read", async () => {
    const settingsRead = deferred<ReturnType<typeof settingsValue>>();
    let settingsSettled = false;
    const { ipc, user } = await renderOnboarding(
      backend({
        get_settings: async () => {
          try {
            return await settingsRead.promise;
          } finally {
            settingsSettled = true;
          }
        },
      }),
    );

    await waitFor(() => expect(ipc.callsTo("get_settings")).toHaveLength(1));
    settingsRead.reject(backendError("unavailable", "settings read failed"));
    await waitFor(() => expect(settingsSettled).toBe(true));

    const english = screen.getByRole("button", { name: "EN English" });
    const finnish = screen.getByRole("button", { name: "FI Suomi" });
    expect(english).toHaveAttribute("aria-pressed", "true");
    expect(finnish).toHaveAttribute("aria-pressed", "false");

    await user.click(screen.getByRole("button", { name: "Continue" }));

    await waitFor(() => expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]));
  });
});

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
  test("offers a clear skip path after a failed download and explains the missing engine", async () => {
    const { ipc, user } = await renderOnboarding(
      backend({
        download_pianissimo_model: () => {
          throw backendError("unavailable", "download failed");
        },
      }),
    );

    await chooseSwedish(user);
    expect(await screen.findByText("download failed")).toBeInTheDocument();
    expect(ipc.callsTo("download_pianissimo_model")).toEqual([{}]);
    await user.click(screen.getByRole("button", { name: "Continue without speech engine" }));
    expect(await screen.findByRole("heading", { name: "You're All Set!" })).toBeInTheDocument();
    expect(screen.getByText(/Speech engine is not installed yet/)).toBeInTheDocument();
  });

  test("Swedish downloads the model the backend recommends (Pianissimo where supported)", async () => {
    const { ipc, user } = await renderOnboarding(backend());
    await chooseSwedish(user);
    expect(await screen.findByText("Speech engine ready")).toBeInTheDocument();
    expect(ipc.callsTo("set_language")).toEqual([{ language: "sv" }]);
    expect(ipc.callsTo("get_effective_model_info")).toEqual([{ language: "sv" }]);
    expect(ipc.callsTo("download_pianissimo_model")).toEqual([{}]);
    expect(ipc.callsTo("download_model")).toEqual([]);
    expect(screen.queryByText("Pianissimo")).not.toBeInTheDocument();
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

test("Accessibility can reopen System Settings while its permission request is pending", async () => {
  const permissionRequest = deferred<void>();
  let granted = false;
  const { ipc, user } = await renderOnboarding(
    backend({
      get_platform: () => "macos",
      check_accessibility_permission: () => granted,
      request_accessibility_permission: () => permissionRequest.promise,
      open_accessibility_settings: () => undefined,
      retry_hotkey_registration: () => undefined,
    }),
  );

  await chooseSwedish(user);
  await user.click(await screen.findByRole("button", { name: "Continue" }));
  await user.click(await screen.findByRole("button", { name: "Skip" }));
  expect(await screen.findByRole("heading", { name: "Accessibility Permission" })).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Open System Settings" }));
  const reopen = await screen.findByRole("button", { name: "Open Accessibility Settings Again" });
  expect(reopen).toBeEnabled();
  const checksBeforeReopen = ipc.callsTo("check_accessibility_permission").length;
  await user.click(reopen);

  await waitFor(() => expect(ipc.callsTo("open_accessibility_settings")).toEqual([{}]));
  expect(ipc.callsTo("request_accessibility_permission")).toEqual([{}]);
  expect(ipc.callsTo("check_accessibility_permission")).toHaveLength(checksBeforeReopen);
  vi.useFakeTimers();
  try {
    granted = true;
    permissionRequest.resolve();
    await vi.advanceTimersByTimeAsync(1000);
    expect(screen.getByText("Accessibility granted")).toBeInTheDocument();
    expect(ipc.callsTo("retry_hotkey_registration")).toEqual([{}]);
  } finally {
    vi.useRealTimers();
  }
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
