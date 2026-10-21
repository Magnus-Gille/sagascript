import { mockWindows } from "@tauri-apps/api/mocks";
import { Window as TauriWindow } from "@tauri-apps/api/window";
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test, vi } from "vitest";
import App from "./App.svelte";
import { deferred } from "./test/deferred";
import { mockIpc } from "./test/ipc";
import { settingsBackend, settingsValue } from "./test/settings";

let originalUrl: string;

afterEach(() => {
  if (originalUrl) window.history.replaceState({}, "", originalUrl);
});

function setWindowUrl(path: string): void {
  originalUrl = window.location.href;
  window.history.replaceState({}, "", path);
}

function onboardingBackend(completed = false) {
  return settingsBackend({
    get_settings: () => settingsValue({ has_completed_onboarding: completed }),
    get_platform: () => "windows",
    microphone_status: () => "unsupported",
    check_accessibility_permission: () => true,
    set_language: () => [],
    get_effective_model_info: () => ({
      id: "pianissimo-sv",
      display_name: "Pianissimo",
      description: "",
      size_mb: 640,
      downloaded: false,
      active: false,
    }),
    download_pianissimo_model: () => undefined,
    download_model: () => undefined,
  });
}

describe("App first-start onboarding", () => {
  test("incomplete settings open the rendered onboarding flow", async () => {
    setWindowUrl("/?keep=first-start");
    const ipc = mockIpc(onboardingBackend());
    mockWindows("main");

    const view = render(App);
    try {
      expect(await screen.findByRole("heading", { name: "Set up Sagascript" })).toBeInTheDocument();
      await waitFor(() => expect(ipc.callsTo("get_settings")).toHaveLength(2));
      expect(screen.queryByRole("heading", { name: "Sagascript" })).not.toBeInTheDocument();
      expect(window.location.search).toBe("?keep=first-start");
    } finally {
      view.unmount();
    }
  });

  test("completion waits for backend acknowledgement before Settings and preserves other URL parameters", async () => {
    setWindowUrl("/?onboarding=1&keep=visible&mode=first-start");
    const completion = deferred<void>();
    let completed = false;
    const ipc = mockIpc({
      ...onboardingBackend(),
      get_settings: () => settingsValue({ has_completed_onboarding: completed }),
      set_onboarding_completed: () => completion.promise,
    });
    mockWindows("main");
    const hideWindow = vi.spyOn(TauriWindow.prototype, "hide");
    const user = userEvent.setup();

    const view = render(App);
    try {
      expect(await screen.findByRole("heading", { name: "Set up Sagascript" })).toBeInTheDocument();
      await waitFor(() => expect(ipc.callsTo("get_settings")).toHaveLength(1));
      expect(ipc.listenerCount("model-download-progress")).toBe(1);
      expect(ipc.listenerCount("model-ready")).toBe(1);

      await user.click(screen.getByRole("button", { name: "Continue" }));
      expect(await screen.findByRole("heading", { name: "Setting up speech engine" })).toBeInTheDocument();
      expect(await screen.findByRole("button", { name: "Continue" })).toBeEnabled();
      await user.click(screen.getByRole("button", { name: "Continue" }));
      expect(await screen.findByRole("heading", { name: "You're All Set!" })).toBeInTheDocument();

      await user.click(screen.getByRole("button", { name: "Start Using Sagascript" }));
      await waitFor(() => expect(ipc.callsTo("set_onboarding_completed")).toEqual([{}]));
      expect(screen.queryByRole("heading", { name: "Sagascript" })).not.toBeInTheDocument();
      expect(window.location.search).toBe("?onboarding=1&keep=visible&mode=first-start");

      completed = true;
      completion.resolve();

      expect(await screen.findByRole("heading", { name: "Sagascript" })).toBeInTheDocument();
      await waitFor(() => expect(ipc.callsTo("get_settings")).toHaveLength(2));
      expect(window.location.search).toBe("?keep=visible&mode=first-start");
      await waitFor(() => {
        expect(ipc.listenerCount("model-download-progress")).toBe(1);
        expect(ipc.listenerCount("model-ready")).toBe(1);
      });
      expect(ipc.unmocked).toEqual([]);
      expect(hideWindow).not.toHaveBeenCalled();
    } finally {
      completed = true;
      completion.resolve();
      view.unmount();
      await waitFor(() => expect(ipc.listeners()).toEqual([]));
    }
  });
});
