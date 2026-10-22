// Rendered Settings coverage for the remaining release surface (#361 B1).
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { renderSettings, settingsBackend } from "../test/settings";

test("Dictate shows the preparing state while the backend loads a speech engine", async () => {
  const { ipc } = await renderSettings(settingsBackend({ start_recording: () => undefined }));

  ipc.emit("state-changed", "loading_model");

  expect(await screen.findByRole("button", { name: "Preparing speech engine..." })).toBeDisabled();

  ipc.emit("state-changed", "idle");
  await waitFor(() => expect(screen.getByRole("button", { name: "Start recording" })).toBeEnabled());
});

test("Advanced keeps decoder controls behind its disclosure while Dictate controls remain available", async () => {
  await renderSettings();

  expect(screen.getByRole("combobox", { name: "Model for Svenska" })).toBeInTheDocument();
  await userEvent.setup().click(screen.getByRole("button", { name: "Settings" }));
  expect(screen.getByRole("switch", { name: "Show recording overlay" })).toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Personal dictionary" })).toBeInTheDocument();
  const user = userEvent.setup();
  await user.click(await screen.findByText("Advanced"));
  const decoding = screen.getByRole("combobox", { name: "Decoding mode" });
  const fallback = screen.getByRole("switch", { name: "Temperature fallback" });
  const vad = screen.getByRole("switch", { name: "Voice activity detection" });
  expect(decoding).toBeVisible();
  expect(fallback).toBeVisible();
  expect(vad).toBeVisible();
  expect(screen.getByRole("switch", { name: "Show recording overlay" })).toBeInTheDocument();

  await user.click(screen.getByText("Advanced"));
  expect(decoding).not.toBeVisible();
  expect(fallback).not.toBeVisible();
  expect(vad).not.toBeVisible();
});
