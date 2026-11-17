// App.svelte routes `?window=record` (the tray's "Record..." window) to the
// recorder and leaves the other windows alone (SAGA-564).
import { render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, describe, expect, test } from "vitest";
import App from "./App.svelte";
import { recorderBackend } from "./test/record";

let originalUrl = "";

afterEach(() => {
  if (originalUrl) window.history.replaceState({}, "", originalUrl);
  originalUrl = "";
});

function setWindowUrl(path: string): void {
  originalUrl = window.location.href;
  window.history.replaceState({}, "", path);
}

describe("App recorder window", () => {
  test("?window=record opens the recorder, not Settings or onboarding", async () => {
    setWindowUrl("/index.html?window=record");
    const backend = recorderBackend();
    render(App);

    expect(await screen.findByRole("heading", { name: "Record" })).toBeInTheDocument();
    await waitFor(() => expect(backend.listenerCount("recording-session-changed")).toBe(1));
    expect(screen.queryByRole("heading", { name: "Set up Sagascript" })).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Sagascript" })).not.toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Start recording" })).toBeInTheDocument();
  });
});
