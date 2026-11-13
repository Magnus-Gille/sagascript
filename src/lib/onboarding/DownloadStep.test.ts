// The onboarding download step on its own: the Onboarding tests cover the
// progress, the completion and the skip after a failure; retrying is pinned here.
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import DownloadStep from "./DownloadStep.svelte";

test("a failed download shows the error and can be retried", async () => {
  const startDownload = vi.fn();
  const nextStep = vi.fn();
  render(DownloadStep, {
    selectedLanguage: "en",
    recommendedSizeMb: null,
    downloadError: "Disk is full",
    downloadComplete: false,
    downloading: false,
    downloadProgress: 0,
    startDownload,
    nextStep,
  });
  expect(screen.getByText("Disk is full")).toBeInTheDocument();
  expect(screen.getByText(/\(142 MB\)/)).toBeInTheDocument();
  await userEvent.setup().click(screen.getByRole("button", { name: "Try Again" }));
  expect(startDownload).toHaveBeenCalledOnce();
  expect(nextStep).not.toHaveBeenCalled();
});
