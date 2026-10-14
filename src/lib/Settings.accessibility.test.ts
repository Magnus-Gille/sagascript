// Settings.svelte on macOS (#362): the two places that need the Accessibility
// permission, Auto-paste and a bare F13 to F24 shortcut, ask for it, wait for
// macOS to report it and only then retry the hotkey registration and save.
import { fireEvent, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import type { CommandHandlers } from "../test/ipc";
import { renderSettings, settingsBackend } from "../test/settings";

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

const POLL = { timeout: 4000 };

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

    await waitFor(() => expect(ipc.callsTo("set_auto_paste")).toEqual([{ enabled: true }]), POLL);
    const commands = ipc.calls.slice(before).map((call) => call.command);
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

    await waitFor(() => expect(ipc.callsTo("set_hotkey_profiles")).toHaveLength(1), POLL);
    const commands = ipc.calls.slice(before).map((call) => call.command);
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
