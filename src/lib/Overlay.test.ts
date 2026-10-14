// Overlay.svelte (the dictation indicator) against the typed IPC mock (#362):
// what it shows from the backend's current state and from live events.
import { render, screen, waitFor } from "@testing-library/svelte";
import { describe, expect, test } from "vitest";
import type { AppState } from "./api";
import Overlay from "./Overlay.svelte";
import { deferred } from "../test/deferred";
import { mockIpc, type CommandHandlers } from "../test/ipc";
import { swedishProfile } from "../test/settings";

function backend(overrides: CommandHandlers = {}): CommandHandlers {
  return {
    get_active_hotkey_profile: () => swedishProfile,
    get_state: () => "recording",
    get_engine_load_state: () => ({ state: "ready" }),
    ...overrides,
  };
}

async function renderOverlay(handlers: CommandHandlers = backend()) {
  const ipc = mockIpc(handlers);
  render(Overlay);
  await waitFor(() => expect(ipc.callsTo("get_engine_load_state")).toHaveLength(1));
  return ipc;
}

const indicator = () => screen.getByRole("status");

describe("dictation indicator", () => {
  test("starts from the backend's current profile and state", async () => {
    await renderOverlay();
    await waitFor(() => expect(indicator()).toHaveTextContent("Recording · Swedish"));
  });

  test("an indicator created while the backend is transcribing says so", async () => {
    await renderOverlay(backend({ get_state: () => "transcribing" }));
    await waitFor(() => expect(indicator()).toHaveTextContent("Transcribing…"));
    expect(indicator()).toHaveAttribute("aria-busy", "true");
  });

  test("follows the state and the profile the backend announces", async () => {
    const ipc = await renderOverlay();
    await waitFor(() => expect(indicator()).toHaveTextContent("Recording · Swedish"));

    ipc.emit("state-changed", "transcribing");
    await waitFor(() => expect(indicator()).toHaveTextContent("Transcribing…"));

    ipc.emit("state-changed", "recording");
    ipc.emit("active-hotkey-profile-changed", { ...swedishProfile, language: "en" });
    await waitFor(() => expect(indicator()).toHaveTextContent("Recording · English"));
  });

  test("a load failure is shown until the next recording", async () => {
    const ipc = await renderOverlay(backend({ get_state: () => "transcribing" }));
    await waitFor(() => expect(indicator()).toHaveTextContent("Transcribing…"));

    ipc.emit("engine-load-state", { state: "failed" });
    await waitFor(() => expect(indicator()).toHaveTextContent("Model failed to load"));
    ipc.emit("state-changed", "idle");
    expect(indicator()).toHaveTextContent("Model failed to load");
  });

  test("an event that arrives while the initial state is fetched is not overwritten by the older answer", async () => {
    const backendState = deferred<AppState>();
    const ipc = mockIpc(backend({ get_state: () => backendState.promise }));
    render(Overlay);
    await waitFor(() => expect(ipc.callsTo("get_state")).toHaveLength(1));

    ipc.emit("state-changed", "transcribing");
    backendState.resolve("recording");

    await waitFor(() => expect(indicator()).toHaveTextContent("Transcribing…"));
    // The fetch settled: give its (discarded) answer the chance to apply.
    await waitFor(() => expect(ipc.callsTo("get_engine_load_state")).toHaveLength(1));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(indicator()).toHaveTextContent("Transcribing…");
  });
});
