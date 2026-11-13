// The onboarding microphone step on its own: the Onboarding tests pass through it
// with access already granted, so its other permission states are pinned here.
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test, vi } from "vitest";
import MicrophoneStep from "./MicrophoneStep.svelte";

function renderStep(micStatus: string) {
  const calls: string[] = [];
  const props = {
    micStatus,
    nextStep: vi.fn(() => calls.push("nextStep")),
    stopPoll: vi.fn(() => calls.push("stopPoll")),
    openMicrophoneSettingsAndPoll: vi.fn(),
    grantMicrophone: vi.fn(),
  };
  render(MicrophoneStep, props);
  return { props, calls, user: userEvent.setup() };
}

const skipLabel = "I don't need this — I'll only transcribe files";

describe("MicrophoneStep", () => {
  test("granted access continues to the next step", async () => {
    const { props, user } = renderStep("authorized");
    expect(screen.getByText("Microphone access granted")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(props.nextStep).toHaveBeenCalledOnce();
  });

  test("not yet asked: the grant button requests access, the skip stops polling first", async () => {
    const { props, calls, user } = renderStep("not_determined");
    expect(screen.getByText("Microphone access not granted")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Grant Microphone Access" }));
    expect(props.grantMicrophone).toHaveBeenCalledOnce();
    await user.click(screen.getByRole("button", { name: skipLabel }));
    expect(calls).toEqual(["stopPoll", "nextStep"]);
  });

  test("while the request is pending the grant button is disabled and waits", () => {
    renderStep("checking");
    const grant = screen.getByRole("button", { name: "Waiting for permission..." });
    expect(grant).toBeDisabled();
  });

  test("denied access opens System Settings or skips after stopping the poll", async () => {
    const { props, calls, user } = renderStep("denied");
    expect(screen.getByText("Permission was denied")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Waiting — Open System Settings" }));
    expect(props.openMicrophoneSettingsAndPoll).toHaveBeenCalledOnce();
    await user.click(screen.getByRole("button", { name: skipLabel }));
    expect(calls).toEqual(["stopPoll", "nextStep"]);
  });

  test.each([
    ["restricted", "Restricted by your organization"],
    ["unsupported", "Permission checks require a built app bundle. Skip to continue."],
  ])("%s access can only be skipped", async (micStatus, text) => {
    const { props, user } = renderStep(micStatus);
    expect(screen.getByText(text)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Skip" }));
    expect(props.nextStep).toHaveBeenCalledOnce();
    expect(props.stopPoll).not.toHaveBeenCalled();
  });
});
