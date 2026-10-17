// Settings.svelte on macOS (#362): the two places that need the Accessibility
// permission, Auto-paste and a bare F13 to F24 shortcut, ask for it, wait for
// macOS to report it and only then retry the hotkey registration and save.
import { fireEvent, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test, vi } from "vitest";
import { backendError, type CommandHandlers } from "../test/ipc";
import { renderSettings, settingsBackend, settingsValue, swedishProfile } from "../test/settings";

// The real wait polls once a second; these tests poll every few milliseconds.
vi.mock("./permissions", async (importOriginal) => {
  const real = await importOriginal<typeof import("./permissions")>();
  return {
    ...real,
    waitForAccessibility: (options = {}) => real.waitForAccessibility({ intervalMs: 5, ...options }),
    requestAccessibility: (options = {}) => real.requestAccessibility({ intervalMs: 5, ...options }),
  };
});

/** The commands in order, a poll's repeated checks counting once. */
const sequence = (commands: string[]) => commands.filter((command, index) => command !== commands[index - 1]);

function macBackend(overrides: CommandHandlers = {}): CommandHandlers {
  return settingsBackend({
    get_platform: () => "macos",
    check_accessibility_permission: () => false,
    request_accessibility_permission: () => undefined,
    retry_hotkey_registration: () => undefined,
    set_auto_paste: () => undefined,
    set_hotkey_profiles: () => [],
    ...overrides,
  });
}

describe("Auto-paste on macOS", () => {
  test("an already granted permission needs no prompt", async () => {
    const { ipc } = await renderSettings(macBackend({ check_accessibility_permission: () => true }));
    const before = ipc.calls.length;

    await userEvent.setup().click(screen.getByRole("switch", { name: "Auto-paste transcription" }));

    await waitFor(() => expect(ipc.callsTo("set_auto_paste")).toEqual([{ enabled: true }]));
    expect(ipc.calls.slice(before).map((call) => call.command)).toEqual([
      "check_accessibility_permission",
      "set_auto_paste",
      "get_settings",
    ]);
  });

  test("asks, waits for the grant, retries the hotkey registration, then saves auto-paste on", async () => {
    let granted = false;
    const { ipc } = await renderSettings(macBackend({ check_accessibility_permission: () => granted }));
    const before = ipc.calls.length;

    await userEvent.setup().click(screen.getByRole("switch", { name: "Auto-paste transcription" }));
    await waitFor(() => expect(ipc.callsTo("request_accessibility_permission")).toHaveLength(1));
    expect(screen.getByRole("switch", { name: "Auto-paste transcription" })).toBeDisabled();
    granted = true;

    await waitFor(() => expect(ipc.callsTo("set_auto_paste")).toEqual([{ enabled: true }]));
    const commands = sequence(ipc.calls.slice(before).map((call) => call.command));
    expect(commands.slice(0, 4)).toEqual([
      "check_accessibility_permission",
      "request_accessibility_permission",
      "check_accessibility_permission",
      "retry_hotkey_registration",
    ]);
    expect(commands.indexOf("set_auto_paste")).toBeGreaterThan(commands.indexOf("retry_hotkey_registration"));
    await waitFor(() => expect(screen.getByRole("switch", { name: "Auto-paste transcription" })).toBeEnabled());
  });

  test("a failing request keeps auto-paste off and shows why", async () => {
    const { ipc } = await renderSettings(
      macBackend({
        request_accessibility_permission: () => {
          throw "System Settings did not open";
        },
      }),
    );

    await userEvent.setup().click(screen.getByRole("switch", { name: "Auto-paste transcription" }));

    await waitFor(() => expect(ipc.callsTo("set_auto_paste")).toEqual([{ enabled: false }]));
    expect(await screen.findByText("System Settings did not open")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Auto-paste transcription" })).toBeEnabled();
  });
});

describe("a bare F13 shortcut on macOS", () => {
  test("is saved once the permission is granted", async () => {
    let granted = false;
    const { ipc } = await renderSettings(macBackend({ check_accessibility_permission: () => granted }));
    await userEvent.setup().click(screen.getByRole("button", { name: "Svenska Press to start/stop shortcut" }));
    const before = ipc.calls.length;

    await fireEvent.keyDown(await screen.findByRole("button", { name: "Svenska Press to start/stop shortcut" }), {
      key: "F13",
      code: "F13",
    });
    await waitFor(() => expect(ipc.callsTo("request_accessibility_permission")).toHaveLength(1));
    granted = true;

    await waitFor(() => expect(ipc.callsTo("set_hotkey_profiles")).toHaveLength(1));
    const commands = sequence(ipc.calls.slice(before).map((call) => call.command));
    expect(commands.slice(0, 4)).toEqual([
      "check_accessibility_permission",
      "request_accessibility_permission",
      "check_accessibility_permission",
      "retry_hotkey_registration",
    ]);
    expect(commands.indexOf("set_hotkey_profiles")).toBeGreaterThan(commands.indexOf("retry_hotkey_registration"));
    expect(ipc.callsTo("set_hotkey_profiles")[0].profiles[0]).toMatchObject({ id: "default", toggle_shortcut: "F13" });
  });

  test("is refused, with the reason, when the request fails", async () => {
    const { ipc } = await renderSettings(
      macBackend({
        request_accessibility_permission: () => {
          throw "System Settings did not open";
        },
      }),
    );
    await userEvent.setup().click(screen.getByRole("button", { name: "Svenska Press to start/stop shortcut" }));

    await fireEvent.keyDown(await screen.findByRole("button", { name: "Svenska Press to start/stop shortcut" }), {
      key: "F13",
      code: "F13",
    });

    expect(await screen.findByText("System Settings did not open")).toBeInTheDocument();
    expect(ipc.callsTo("set_hotkey_profiles")).toEqual([]);
  });
});

describe("the hotkey registration retry on macOS", () => {
  // A bare F13 is saved, Accessibility is granted, but the registration is not
  // healthy: Settings retries it when it opens.
  const f13 = settingsValue({
    hotkey: "F13",
    hotkey_profiles: [{ ...swedishProfile, shortcut: "F13", push_to_talk_shortcut: null, toggle_shortcut: "F13" }],
  });
  const unhealthy = {
    ok: false,
    error: "The macOS F13-F24 event monitor is unavailable; restart Sagascript",
    shortcut: "F13",
    shortcuts: ["F13"],
    settings_watcher: { running: true, recovered_panics: 0 },
  };
  const backend = (retry: CommandHandlers["retry_hotkey_registration"]) =>
    macBackend({
      get_settings: () => f13,
      check_accessibility_permission: () => true,
      hotkey_status: () => unhealthy,
      retry_hotkey_registration: retry,
    });

  test("a retry that was refused as busy says to try again, not that the monitor is unavailable", async () => {
    const { ipc } = await renderSettings(
      backend(() => {
        throw backendError("busy", "The macOS F13-F24 event monitor is still starting; try again in a moment", true);
      }),
    );

    await waitFor(() => expect(ipc.callsTo("retry_hotkey_registration")).toHaveLength(1));
    expect(await screen.findByText(/still starting; try again in a moment/)).toBeInTheDocument();
    expect(screen.queryByText(/restart Sagascript/)).not.toBeInTheDocument();
  });

  test("a retry that failed to register leaves the registration health's diagnostic", async () => {
    const { ipc } = await renderSettings(
      backend(() => {
        throw backendError("hotkey_change_failed", "Failed to register hotkey profiles: shortcut in use");
      }),
    );

    await waitFor(() => expect(ipc.callsTo("retry_hotkey_registration")).toHaveLength(1));
    expect(await screen.findByText(/event monitor is unavailable; restart Sagascript/)).toBeInTheDocument();
    expect(screen.queryByText(/shortcut in use/)).not.toBeInTheDocument();
  });
});
