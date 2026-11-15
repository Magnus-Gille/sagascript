// QueueStatus on its own (SAGA-379): the status line that counts the file
// queue's jobs by status and the notice for a job that needs a status check.
// The same area through the whole window: src/lib/Settings.test.ts and
// src/lib/Settings.downloads.test.ts.
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import { renderPanel } from "../../test/settingsPanel";
import QueueStatus from "./QueueStatus.svelte";

const statusRegion = () => screen.getByRole("status", { name: "File transcription status" });
const normalized = (element: HTMLElement) => element.textContent?.replace(/\s+/g, " ").trim();

describe("the queue summary", () => {
  test("is an empty status region while the queue has no jobs", async () => {
    await renderPanel(QueueStatus);

    expect(statusRegion()).toBeEmptyDOMElement();
  });

  test("counts the jobs by status as they run and complete", async () => {
    const { win } = await renderPanel(QueueStatus);

    // The window starts the first queued job by itself; the second waits behind it.
    win.queue.addFiles(["/audio/one.wav", "/audio/two.wav"]);
    await waitFor(() =>
      expect(normalized(statusRegion())).toBe("0 completed · 0 failed · 0 cancelled · 1 queued · 1 running · 0 needs retry"),
    );

    // Completing it hands the run to the job behind it.
    win.queue.complete(win.queue.jobs[0].id, "completed");
    await waitFor(() =>
      expect(normalized(statusRegion())).toBe("1 completed · 0 failed · 0 cancelled · 0 queued · 1 running · 0 needs retry"),
    );
  });
});

describe("a job that needs a status check", () => {
  test("names the file and offers a button that shows it", async () => {
    const { win } = await renderPanel(QueueStatus);
    win.queue.addFiles(["/audio/one.wav"]);

    win.queue.setAttention(win.queue.jobs[0].id, true);

    expect(await screen.findByText("one.wav needs a status check.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show one.wav" })).toBeInTheDocument();
    expect(screen.queryByText(/The queue is paused\./)).toBeNull();
    expect(normalized(statusRegion())).toContain("1 needs retry");
  });

  test("says the queue is paused while another job is still queued", async () => {
    const { win } = await renderPanel(QueueStatus);
    win.queue.addFiles(["/audio/one.wav", "/audio/two.wav"]);

    win.queue.setAttention(win.queue.jobs[0].id, true);

    const notice = await screen.findByText(/one\.wav needs a status check\./);
    // Compared with all whitespace removed: today the two sentences render with no space between them,
    // and this test pins the wording, not that.
    expect(normalized(notice)?.replace(/\s/g, "")).toBe("one.wavneedsastatuscheck.Thequeueispaused.");
  });

  test("pressing the button asks the router for the Transcribe tab and selects that job", async () => {
    const user = userEvent.setup();
    const { win } = await renderPanel(QueueStatus);
    win.queue.addFiles(["/audio/one.wav", "/audio/two.wav"]);
    const [first, second] = win.queue.jobs;
    expect(win.router.active).toBe("dictate");
    expect(win.queue.selectedId).toBe(first.id);

    win.queue.setAttention(second.id, true);
    await user.click(await screen.findByRole("button", { name: "Show two.wav" }));

    expect(win.router.active).toBe("transcribe");
    expect(win.queue.selectedId).toBe(second.id);
  });

  test("the button does not change the tab while a panel holds the tabs", async () => {
    const user = userEvent.setup();
    const { win } = await renderPanel(QueueStatus);
    win.queue.addFiles(["/audio/one.wav", "/audio/two.wav"]);
    const [first, second] = win.queue.jobs;
    win.queue.setAttention(second.id, true);
    win.router.held = true;

    await user.click(await screen.findByRole("button", { name: "Show two.wav" }));

    expect(win.router.active).toBe("dictate");
    expect(win.queue.selectedId).toBe(first.id);
  });
});
