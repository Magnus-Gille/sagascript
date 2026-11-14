// The error of each tab through the whole window (SAGA-379). A settings change
// that fails is reported on the tab where it was made: the error box shows it
// while that tab shows, the tab's button is marked while another tab shows, and
// a change on another tab neither shows nor clears it. What belongs to the
// window (a failed discard of recovered drafts) shows on every tab.
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import { createUpdateRecoveryPayload } from "./update-recovery";
import { settle } from "../test/deferred";
import { backendError, type CommandHandlers } from "../test/ipc";
import { pianissimo, renderSettings, settingsBackend, settingsValue } from "../test/settings";

const AUTO_PASTE_FAILED = "Could not save auto-paste.";
const OVERLAY_FAILED = "Could not save the overlay.";
const ENGINE_CHECK_FAILED = "The speech engines could not be listed.";
const DISCARD_FAILED = "Could not discard recovered drafts: Permission denied";

type TabLabel = "Dictate" | "Transcribe" | "Settings";

/** The tab's button while it carries no mark. */
const tab = (label: TabLabel) => screen.getByRole("button", { name: label });
/** The tab's button while it is marked as having an error. */
const markedTab = (label: TabLabel) => screen.queryByRole("button", { name: `${label}, has an error` });
const autoPaste = () => screen.findByRole("switch", { name: "Auto-paste transcription" });
const overlay = () => screen.findByRole("switch", { name: "Show recording overlay" });
const overlayIsOff = () =>
  waitFor(() => expect(screen.getByRole("switch", { name: "Show recording overlay" })).toHaveAttribute("aria-checked", "false"));

/** A backend whose auto-paste and overlay changes fail, each with its own message; `accepted` names the ones that are saved instead. */
function backend(accepted: { autoPaste?: boolean; overlay?: boolean } = {}, overrides: CommandHandlers = {}): CommandHandlers {
  let current = settingsValue();
  return settingsBackend({
    get_settings: () => current,
    set_auto_paste: ({ enabled }) => {
      if (!accepted.autoPaste) throw backendError("io", AUTO_PASTE_FAILED);
      current = { ...current, auto_paste: enabled };
    },
    set_show_overlay: ({ enabled }) => {
      if (!accepted.overlay) throw backendError("io", OVERLAY_FAILED);
      current = { ...current, show_overlay: enabled };
    },
    ...overrides,
  });
}

describe("a change that fails on the Dictate tab", () => {
  test("is reported on Dictate, not on the other tabs, and again when Dictate is shown", async () => {
    await renderSettings(backend());
    const user = userEvent.setup();

    await user.click(await autoPaste());
    expect(await screen.findByRole("alert")).toHaveTextContent(AUTO_PASTE_FAILED);

    await user.click(tab("Transcribe"));
    await screen.findByRole("button", { name: "Open Files..." });
    expect(screen.queryByText(AUTO_PASTE_FAILED)).toBeNull();

    await user.click(tab("Settings"));
    await overlay();
    expect(screen.queryByText(AUTO_PASTE_FAILED)).toBeNull();

    await user.click(markedTab("Dictate")!);
    expect(await screen.findByRole("alert")).toHaveTextContent(AUTO_PASTE_FAILED);
  });

  test("marks the Dictate button while another tab shows, and not while Dictate shows", async () => {
    await renderSettings(backend());
    const user = userEvent.setup();
    await user.click(await autoPaste());
    await screen.findByText(AUTO_PASTE_FAILED);
    expect(markedTab("Dictate")).toBeNull();
    expect(tab("Dictate")).toBeInTheDocument();

    await user.click(tab("Settings"));
    await overlay();

    expect(markedTab("Dictate")).toBeInTheDocument();
    expect(markedTab("Settings")).toBeNull();
    expect(markedTab("Transcribe")).toBeNull();

    await user.click(markedTab("Dictate")!);
    await screen.findByText(AUTO_PASTE_FAILED);
    expect(markedTab("Dictate")).toBeNull();
    expect(tab("Dictate")).toBeInTheDocument();
  });

  test("stays when a change on the Settings tab is saved", async () => {
    const { ipc } = await renderSettings(backend({ overlay: true }));
    const user = userEvent.setup();
    await user.click(await autoPaste());
    await screen.findByText(AUTO_PASTE_FAILED);

    await user.click(tab("Settings"));
    await user.click(await overlay());
    await waitFor(() => expect(ipc.callsTo("set_show_overlay")).toEqual([{ enabled: false }]));
    await overlayIsOff();

    expect(markedTab("Dictate")).toBeInTheDocument();
    await user.click(markedTab("Dictate")!);
    expect(await screen.findByRole("alert")).toHaveTextContent(AUTO_PASTE_FAILED);
  });

  test("is cleared by the next change on Dictate that is saved", async () => {
    let fail = true;
    const { ipc } = await renderSettings(
      backend(
        {},
        {
          set_auto_paste: () => {
            if (fail) throw backendError("io", AUTO_PASTE_FAILED);
          },
        },
      ),
    );
    const user = userEvent.setup();
    await user.click(await autoPaste());
    await screen.findByText(AUTO_PASTE_FAILED);

    fail = false;
    await user.click(await autoPaste());

    await waitFor(() => expect(ipc.callsTo("set_auto_paste")).toHaveLength(2));
    await waitFor(() => expect(screen.queryByText(AUTO_PASTE_FAILED)).toBeNull());
  });
});

describe("a failed change on each of two tabs", () => {
  test("leaves each tab its own error", async () => {
    await renderSettings(backend());
    const user = userEvent.setup();
    await user.click(await autoPaste());
    await screen.findByText(AUTO_PASTE_FAILED);

    await user.click(tab("Settings"));
    await user.click(await overlay());

    expect(await screen.findByRole("alert")).toHaveTextContent(OVERLAY_FAILED);
    expect(screen.queryByText(AUTO_PASTE_FAILED)).toBeNull();
    expect(markedTab("Dictate")).toBeInTheDocument();
    expect(markedTab("Settings")).toBeNull();

    await user.click(markedTab("Dictate")!);

    expect(await screen.findByRole("alert")).toHaveTextContent(AUTO_PASTE_FAILED);
    expect(screen.queryByText(OVERLAY_FAILED)).toBeNull();
    expect(markedTab("Settings")).toBeInTheDocument();
    expect(markedTab("Dictate")).toBeNull();
  });
});

describe("a failed check of the speech engines", () => {
  /** The check fails while `failing` is set; it runs when the window loads and on `model-ready`. */
  function engineBackend() {
    const state = { failing: false };
    const handlers = backend(
      {},
      {
        get_profile_model_info: () => {
          if (state.failing) throw backendError("io", ENGINE_CHECK_FAILED);
          return pianissimo;
        },
      },
    );
    return { state, handlers };
  }

  test("that happens while Settings shows marks Dictate and is reported there, not on Settings", async () => {
    const { state, handlers } = engineBackend();
    const { ipc } = await renderSettings(handlers);
    const user = userEvent.setup();
    await user.click(tab("Settings"));
    await overlay();

    state.failing = true;
    ipc.emit("model-ready", null);

    await waitFor(() => expect(markedTab("Dictate")).toBeInTheDocument());
    expect(screen.queryByText(ENGINE_CHECK_FAILED)).toBeNull();

    await user.click(markedTab("Dictate")!);
    expect(await screen.findByRole("alert")).toHaveTextContent(ENGINE_CHECK_FAILED);
  });

  test("is lifted by the next check that succeeds", async () => {
    const { state, handlers } = engineBackend();
    const { ipc } = await renderSettings(handlers);
    await autoPaste();
    state.failing = true;
    ipc.emit("model-ready", null);
    expect(await screen.findByRole("alert")).toHaveTextContent(ENGINE_CHECK_FAILED);

    state.failing = false;
    ipc.emit("model-ready", null);

    await waitFor(() => expect(screen.queryByText(ENGINE_CHECK_FAILED)).toBeNull());
  });

  test("leaves another Dictate error in place when a later check succeeds", async () => {
    const { state, handlers } = engineBackend();
    const { ipc } = await renderSettings(handlers);
    const user = userEvent.setup();
    state.failing = true;
    ipc.emit("model-ready", null);
    await screen.findByText(ENGINE_CHECK_FAILED);
    await user.click(await autoPaste());
    await screen.findByText(AUTO_PASTE_FAILED);

    state.failing = false;
    const checksBefore = ipc.callsTo("get_effective_model_info").length;
    ipc.emit("model-ready", null);
    // The recommended model is the last thing a check asks for.
    await waitFor(() => expect(ipc.callsTo("get_effective_model_info").length).toBeGreaterThan(checksBefore));
    await settle(20);

    expect(screen.getByRole("alert")).toHaveTextContent(AUTO_PASTE_FAILED);
  });

  test("does not replace the dictionary conflict shown on Settings", async () => {
    const CONFLICT = "Dictionary changed elsewhere: the saved source no longer matches the editor baseline";
    const { state, handlers } = engineBackend();
    const { ipc } = await renderSettings({
      ...handlers,
      set_profile_glossary: () => {
        throw backendError("dictionary_changed_elsewhere", CONFLICT);
      },
    });
    const user = userEvent.setup();
    await user.click(tab("Settings"));
    await user.type(await screen.findByRole("textbox", { name: "Personal dictionary" }), "merge = merch");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(CONFLICT);
    expect(screen.getByText(/This dictionary changed elsewhere\. Your draft is preserved/)).toBeInTheDocument();

    state.failing = true;
    ipc.emit("model-ready", null);

    await waitFor(() => expect(markedTab("Dictate")).toBeInTheDocument());
    expect(screen.getByRole("alert")).toHaveTextContent(CONFLICT);
    expect(screen.getByText(/This dictionary changed elsewhere\. Your draft is preserved/)).toBeInTheDocument();
  });
});

describe("a failed discard of the recovered drafts", () => {
  const drafts = createUpdateRecoveryPayload(
    { dictation: { text: "Osparad diktering" }, files: [], meetings: [] },
    "2026-10-13T08:00:00.000Z",
  );

  /** Stored drafts whose deletion fails while `failing` is set. */
  function recoveryBackend(accepted: { overlay?: boolean } = {}) {
    const state = { failing: true };
    let stored: typeof drafts | null = drafts;
    const handlers = backend(accepted, {
      load_update_recovery: () => stored,
      clear_update_recovery: () => {
        if (state.failing) throw backendError("io", "Permission denied");
        stored = null;
      },
    });
    return { state, handlers };
  }

  const discard = () => screen.findByRole("button", { name: "Discard recovered drafts" });

  test("is reported on every tab and marks none of them", async () => {
    await renderSettings(recoveryBackend().handlers);
    const user = userEvent.setup();

    await user.click(await discard());
    expect(await screen.findByText(DISCARD_FAILED)).toHaveAttribute("role", "alert");

    for (const label of ["Transcribe", "Settings", "Dictate"] as const) {
      await user.click(tab(label));
      expect(screen.getByText(DISCARD_FAILED)).toBeInTheDocument();
    }
    expect(markedTab("Dictate")).toBeNull();
    expect(markedTab("Transcribe")).toBeNull();
    expect(markedTab("Settings")).toBeNull();
  });

  test("stays when a setting is saved, next to a tab's own error, and goes when the drafts are discarded", async () => {
    const { state, handlers } = recoveryBackend({ overlay: true });
    const { ipc } = await renderSettings(handlers);
    const user = userEvent.setup();
    await user.click(await discard());
    await screen.findByText(DISCARD_FAILED);

    await user.click(tab("Settings"));
    await user.click(await overlay());
    await waitFor(() => expect(ipc.callsTo("set_show_overlay")).toEqual([{ enabled: false }]));
    await overlayIsOff();
    expect(screen.getByText(DISCARD_FAILED)).toBeInTheDocument();

    await user.click(tab("Dictate"));
    await user.click(await autoPaste());
    expect(await screen.findByText(AUTO_PASTE_FAILED)).toBeInTheDocument();
    expect(screen.getByText(DISCARD_FAILED)).toBeInTheDocument();

    state.failing = false;
    await user.click(await discard());

    await waitFor(() => expect(screen.queryByText(DISCARD_FAILED)).toBeNull());
    expect(screen.queryByRole("status", { name: "Recovered drafts" })).toBeNull();
    expect(screen.getByText(AUTO_PASTE_FAILED)).toBeInTheDocument();
  });
});
