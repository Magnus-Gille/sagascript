// AdvancedPanel on its own (SAGA-379): the decoding, temperature fallback and
// voice activity controls, the Pianissimo controls that depend on the engine
// status, and the lock while the settings are unconfirmed. The same flows
// through the whole window: Settings.saving.test.ts and Settings.readback.test.ts.
import { screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import type { Settings as SettingsValue } from "../api";
import { settle } from "../../test/deferred";
import { backendError, type CommandHandlers } from "../../test/ipc";
import { renderPanel } from "../../test/settingsPanel";
import { engineStatus, settingsBackend, settingsValue } from "../../test/settings";
import AdvancedPanel from "./AdvancedPanel.svelte";

/** Render the panel with the "Advanced" section open, over settings `initial` and any further `handlers`. */
async function openAdvanced(initial: Partial<SettingsValue> = {}, handlers: CommandHandlers = {}) {
  const accepted = {
    set_beam_size: () => {},
    set_temperature_fallback: () => {},
    set_vad_enabled: () => {},
    set_engine_prewarm: () => {},
    set_engine_idle_unload_minutes: () => {},
  };
  const rendered = await renderPanel(
    AdvancedPanel,
    settingsBackend({ get_settings: () => settingsValue(initial), ...accepted, ...handlers }),
  );
  const user = userEvent.setup();
  await user.click(screen.getByText("Advanced"));
  return { ...rendered, user };
}

const decoding = () => screen.getByRole("combobox", { name: "Decoding mode" });
const temperature = () => screen.getByRole("switch", { name: "Temperature fallback" });
const vad = () => screen.getByRole("switch", { name: "Voice activity detection" });
const prewarm = () => screen.findByRole("combobox", { name: "Prepare Pianissimo when" });
const idle = () => screen.findByRole("combobox", { name: "Unload after idle" });

describe("the Advanced section", () => {
  test("starts closed and opens when its summary is pressed", async () => {
    await renderPanel(AdvancedPanel, settingsBackend());
    const details = screen.getByText("Advanced").closest("details");
    expect(details).not.toHaveAttribute("open");

    await userEvent.setup().click(screen.getByText("Advanced"));

    expect(details).toHaveAttribute("open");
  });
});

describe("decoding mode", () => {
  test("sends set_beam_size with the number of the chosen mode", async () => {
    const { ipc, user } = await openAdvanced({ beam_size: 5 });
    expect(decoding()).toHaveDisplayValue("Beam search (accurate)");

    await user.selectOptions(decoding(), "Greedy (fast)");

    await waitFor(() => expect(ipc.callsTo("set_beam_size")).toEqual([{ beamSize: 0 }]));
  });

  test("shows the saved mode again when the backend rejects the change", async () => {
    const { ipc, win, user } = await openAdvanced(
      { beam_size: 5 },
      {
        set_beam_size: () => {
          throw backendError("settings_storage_failed", "disk full");
        },
      },
    );

    await user.selectOptions(decoding(), "Greedy (fast)");

    await waitFor(() => expect(win.session.error).toBe("disk full"));
    expect(ipc.callsTo("set_beam_size")).toEqual([{ beamSize: 0 }]);
    expect(decoding()).toHaveDisplayValue("Beam search (accurate)");
  });
});

describe("the temperature fallback and voice activity detection switches", () => {
  test("each sends the opposite of the saved value", async () => {
    const { ipc, user } = await openAdvanced({ temperature_fallback: true, vad_enabled: false });
    expect(temperature()).toHaveAttribute("aria-checked", "true");
    expect(vad()).toHaveAttribute("aria-checked", "false");

    await user.click(temperature());
    await user.click(vad());

    await waitFor(() => expect(ipc.callsTo("set_vad_enabled")).toEqual([{ enabled: true }]));
    expect(ipc.callsTo("set_temperature_fallback")).toEqual([{ enabled: false }]);
  });
});

describe("the Pianissimo controls", () => {
  test("are not shown when the engine status does not report support", async () => {
    const { ipc } = await openAdvanced({}, { engine_status: () => ({ ...engineStatus, supported: false }) });
    await waitFor(() => expect(ipc.callsTo("engine_status").length).toBeGreaterThan(0));
    await settle();

    expect(screen.queryByRole("combobox", { name: "Prepare Pianissimo when" })).toBeNull();
    expect(screen.queryByRole("combobox", { name: "Unload after idle" })).toBeNull();
  });

  test("are shown when the engine status reports support, and each sends its command", async () => {
    const { ipc, user } = await openAdvanced({ engine_prewarm: "on_key_down", engine_idle_unload_minutes: 10 });
    expect(await prewarm()).toHaveDisplayValue("When I press the dictation key");
    expect(await idle()).toHaveDisplayValue("10 minutes");

    await user.selectOptions(await prewarm(), "Only when needed");
    await user.selectOptions(await idle(), "Never");

    await waitFor(() => expect(ipc.callsTo("set_engine_idle_unload_minutes")).toEqual([{ minutes: 0 }]));
    expect(ipc.callsTo("set_engine_prewarm")).toEqual([{ mode: "off" }]);
  });

  test("list a saved idle time that is not a standard choice as its own option", async () => {
    await openAdvanced({ engine_idle_unload_minutes: 45 });

    const select = await idle();
    expect(select).toHaveDisplayValue("45 minutes");
    expect(within(select).getAllByRole("option", { name: "45 minutes" })).toHaveLength(1);
    expect(
      within(select)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual(["5 minutes", "10 minutes", "30 minutes", "60 minutes", "Never", "45 minutes"]);
  });

  test("do not offer an extra option for a standard idle time", async () => {
    await openAdvanced({ engine_idle_unload_minutes: 30 });

    expect(within(await idle()).getAllByRole("option")).toHaveLength(5);
  });
});

describe("while the settings are unconfirmed", () => {
  test("every control in the section is disabled", async () => {
    const { win } = await openAdvanced();
    await prewarm();
    const controls = () => [
      decoding(),
      temperature(),
      vad(),
      screen.getByRole("combobox", { name: "Prepare Pianissimo when" }),
      screen.getByRole("combobox", { name: "Unload after idle" }),
    ];
    expect(controls().filter((control) => control.hasAttribute("disabled"))).toEqual([]);

    win.session.markUnconfirmed("settings read timed out", false);

    await waitFor(() => expect(controls().filter((control) => !control.hasAttribute("disabled"))).toEqual([]));
    expect(controls()).toHaveLength(5);
  });
});
