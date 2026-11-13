// The macOS microphone step of Onboarding.svelte, reached through the flow: what the step
// shows while the request is pending and after macOS answers, and where its buttons lead.
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import Onboarding from "./Onboarding.svelte";
import { deferred } from "../test/deferred";
import { mockIpc, type CommandHandlers } from "../test/ipc";
import { pianissimo, settingsValue } from "../test/settings";

/** macOS that has not been asked for the microphone yet. */
function backend(overrides: CommandHandlers = {}): CommandHandlers {
  return {
    get_platform: () => "macos",
    microphone_status: () => "not_determined",
    request_microphone_access: () => "authorized",
    open_microphone_settings: () => undefined,
    check_accessibility_permission: () => false,
    get_settings: () => settingsValue({ language: "en", has_completed_onboarding: false }),
    set_language: () => [],
    get_effective_model_info: () => pianissimo,
    download_pianissimo_model: () => undefined,
    download_model: () => undefined,
    ...overrides,
  };
}

async function reachMicrophoneStep(handlers: CommandHandlers) {
  const ipc = mockIpc(handlers);
  render(Onboarding, { oncomplete: vi.fn() });
  const user = userEvent.setup();
  await waitFor(() => expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled());
  await user.click(screen.getByRole("button", { name: "Continue" })); // language
  await user.click(await screen.findByRole("button", { name: "Continue" })); // speech engine ready
  await screen.findByRole("heading", { name: "Microphone Access" });
  return { ipc, user };
}

test("a pending request disables its button until macOS answers, and a grant leads on", async () => {
  const request = deferred<string>();
  const { ipc, user } = await reachMicrophoneStep(backend({ request_microphone_access: () => request.promise }));
  expect(screen.getByText("Microphone access not granted")).toBeInTheDocument();

  await user.click(await screen.findByRole("button", { name: "Grant Microphone Access" }));
  const waiting = await screen.findByRole("button", { name: "Waiting for permission..." });
  expect(waiting).toBeDisabled();
  expect(ipc.callsTo("request_microphone_access")).toHaveLength(1);

  request.resolve("authorized");
  expect(await screen.findByText("Microphone access granted")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Waiting for permission..." })).not.toBeInTheDocument();
  expect(ipc.callsTo("open_microphone_settings")).toEqual([]);

  await user.click(screen.getByRole("button", { name: "Continue" }));
  expect(await screen.findByRole("heading", { name: "Accessibility Permission" })).toBeInTheDocument();
});

test("a denied request opens System Settings, and the file-only choice leads on", async () => {
  const { ipc, user } = await reachMicrophoneStep(backend({ request_microphone_access: () => "denied" }));

  await user.click(await screen.findByRole("button", { name: "Grant Microphone Access" }));
  expect(await screen.findByText("Permission was denied")).toBeInTheDocument();
  await waitFor(() => expect(ipc.callsTo("open_microphone_settings")).toHaveLength(1));
  expect(screen.getByRole("button", { name: "Waiting — Open System Settings" })).toBeEnabled();

  await user.click(screen.getByRole("button", { name: "I don't need this — I'll only transcribe files" }));
  expect(await screen.findByRole("heading", { name: "Accessibility Permission" })).toBeInTheDocument();
});
