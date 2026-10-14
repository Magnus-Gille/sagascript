// The macOS Accessibility step of Onboarding.svelte (#362): ask for the
// permission, poll until macOS reports it, then retry the saved hotkey.
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test, vi } from "vitest";
import Onboarding from "./Onboarding.svelte";
import { mockIpc, type CommandHandlers } from "../test/ipc";
import { pianissimo, settingsValue } from "../test/settings";

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

/** macOS with the microphone granted, so the flow reaches the Accessibility step. */
function backend(overrides: CommandHandlers = {}): CommandHandlers {
  return {
    get_platform: () => "macos",
    microphone_status: () => "authorized",
    check_accessibility_permission: () => false,
    request_accessibility_permission: () => undefined,
    retry_hotkey_registration: () => undefined,
    set_auto_paste: () => undefined,
    get_settings: () => settingsValue({ language: "en", has_completed_onboarding: false }),
    set_language: () => [],
    get_effective_model_info: () => pianissimo,
    download_pianissimo_model: () => undefined,
    download_model: () => undefined,
    ...overrides,
  };
}

async function reachAccessibilityStep(handlers: CommandHandlers) {
  const ipc = mockIpc(handlers);
  const view = render(Onboarding, { oncomplete: vi.fn() });
  const user = userEvent.setup();
  await waitFor(() => expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled());
  await user.click(screen.getByRole("button", { name: "Continue" })); // language
  await user.click(await screen.findByRole("button", { name: "Continue" })); // speech engine ready
  await user.click(await screen.findByRole("button", { name: "Continue" })); // microphone granted
  await screen.findByRole("heading", { name: "Accessibility Permission" });
  return { ipc, user, view };
}

const accessibilityCommands = (ipc: ReturnType<typeof mockIpc>) =>
  ipc.calls
    .map((call) => call.command)
    .filter((command) =>
      ["check_accessibility_permission", "request_accessibility_permission", "retry_hotkey_registration", "set_auto_paste"].includes(
        command,
      ),
    );

describe("the Accessibility step", () => {
  test("asks for the permission, polls until it is granted and then retries the saved shortcut", async () => {
    let granted = false;
    const { ipc, user } = await reachAccessibilityStep(backend({ check_accessibility_permission: () => granted }));
    expect(screen.getByText("Accessibility not granted")).toBeInTheDocument();
    const before = ipc.calls.length;

    await user.click(screen.getByRole("button", { name: "Open System Settings" }));
    expect(await screen.findByText("Waiting for permission in System Settings")).toBeInTheDocument();
    expect(ipc.callsTo("request_accessibility_permission")).toHaveLength(1);
    granted = true;

    expect(await screen.findByText("Accessibility granted")).toBeInTheDocument();
    await waitFor(() => expect(ipc.callsTo("retry_hotkey_registration")).toHaveLength(1));
    expect(sequence(ipc.calls.slice(before).map((call) => call.command))).toEqual([
      "request_accessibility_permission",
      "check_accessibility_permission",
      "retry_hotkey_registration",
    ]);

    await user.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(ipc.callsTo("set_auto_paste")).toEqual([{ enabled: true }]));
  });

  test("a permission macOS already reports is shown as granted, without a prompt", async () => {
    const { ipc } = await reachAccessibilityStep(backend({ check_accessibility_permission: () => true }));

    expect(screen.getByText("Accessibility granted")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled();
    expect(ipc.callsTo("request_accessibility_permission")).toEqual([]);
  });

  test("a request that fails says so and offers the request again", async () => {
    const { user } = await reachAccessibilityStep(
      backend({
        request_accessibility_permission: () => {
          throw "System Settings did not open";
        },
      }),
    );

    await user.click(screen.getByRole("button", { name: "Open System Settings" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("System Settings did not open");
    expect(screen.getByRole("button", { name: "Open System Settings" })).toBeInTheDocument();
  });

  test("a failing check ends the wait with its message", async () => {
    let failing = false;
    const { ipc, user } = await reachAccessibilityStep(
      backend({
        check_accessibility_permission: () => {
          if (failing) throw "TCC database unavailable";
          return false;
        },
      }),
    );
    await user.click(screen.getByRole("button", { name: "Open System Settings" }));
    failing = true;

    expect(await screen.findByRole("alert")).toHaveTextContent("TCC database unavailable");
    expect(screen.getByRole("button", { name: "Open System Settings" })).toBeInTheDocument();
    const checks = ipc.callsTo("check_accessibility_permission").length;
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(ipc.callsTo("check_accessibility_permission")).toHaveLength(checks);
  });

  test("a shortcut that cannot be registered after the grant is reported, the grant stands", async () => {
    let granted = false;
    const { user } = await reachAccessibilityStep(
      backend({
        check_accessibility_permission: () => granted,
        retry_hotkey_registration: () => {
          throw "shortcut in use";
        },
      }),
    );
    await user.click(screen.getByRole("button", { name: "Open System Settings" }));
    granted = true;

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Accessibility was granted, but the saved dictation shortcut could not be registered: shortcut in use",
    );
    expect(screen.getByText("Accessibility granted")).toBeInTheDocument();
  });

  test("choosing to paste manually stops the wait and saves auto-paste off", async () => {
    const { ipc, user } = await reachAccessibilityStep(backend());
    await user.click(screen.getByRole("button", { name: "Open System Settings" }));
    await user.click(await screen.findByRole("button", { name: "I'll paste manually" }));
    await waitFor(() => expect(ipc.callsTo("set_auto_paste")).toEqual([{ enabled: false }]));
    const checks = ipc.callsTo("check_accessibility_permission").length;

    await new Promise((resolve) => setTimeout(resolve, 60));

    expect(ipc.callsTo("check_accessibility_permission")).toHaveLength(checks);
    expect(accessibilityCommands(ipc).at(-1)).toBe("set_auto_paste");
  });

  test("leaving the window stops the wait", async () => {
    const { ipc, user, view } = await reachAccessibilityStep(backend());
    await user.click(screen.getByRole("button", { name: "Open System Settings" }));
    await screen.findByText("Waiting for permission in System Settings");

    view.unmount();
    const checks = ipc.callsTo("check_accessibility_permission").length;
    await new Promise((resolve) => setTimeout(resolve, 60));

    expect(ipc.callsTo("check_accessibility_permission")).toHaveLength(checks);
  });
});
