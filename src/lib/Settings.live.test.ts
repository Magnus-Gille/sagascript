// Live meeting navigation is intentionally tested through Settings so a tab
// switch cannot accidentally discard a backend-owned session.
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import type { LiveSnapshot } from "./live-meeting-types";
import { renderSettings, settingsBackend } from "../test/settings";

const restoredSession: LiveSnapshot = {
  id: "session-1",
  revision: 2,
  phase: "running",
  elapsed_ms: 1_000,
  sources: [],
  segments: [
    {
      id: "segment-1",
      source: "microphone",
      start_ms: 0,
      end_ms: 500,
      text: "Mötet fortsätter",
      status: "final",
      revision: 1,
    },
  ],
  gaps: [],
  error: null,
  audio_available: true,
};

test("restores the backend live session after leaving and returning to the tab", async () => {
  const { ipc } = await renderSettings(
    settingsBackend({
      live_meeting_supported: () => true,
      get_live_meeting: () => restoredSession,
    }),
  );
  const user = userEvent.setup();

  await user.click(screen.getByRole("button", { name: "Live meeting" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Stop live meeting" })).toBeInTheDocument());
  expect(screen.getByText("Mötet fortsätter")).toBeInTheDocument();
  const firstLoadCount = ipc.callsTo("get_live_meeting").length;
  expect(firstLoadCount).toBeGreaterThan(0);

  await user.click(screen.getByRole("button", { name: "Dictate" }));
  await waitFor(() => expect(screen.queryByRole("button", { name: "Stop live meeting" })).not.toBeInTheDocument());

  await user.click(screen.getByRole("button", { name: "Live meeting" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Stop live meeting" })).toBeInTheDocument());
  expect(screen.getByText("Mötet fortsätter")).toBeInTheDocument();
  expect(ipc.callsTo("get_live_meeting").length).toBeGreaterThan(firstLoadCount);
  expect(ipc.unmocked).toEqual([]);
});
