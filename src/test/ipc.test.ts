import { invoke, SERIALIZE_TO_IPC_FN } from "@tauri-apps/api/core";
import { emit, listen, once } from "@tauri-apps/api/event";
import { describe, expect, test } from "vitest";
import { getPlatform, getSettings, setHotkeyProfiles, setLanguage, setProfileGlossary, type Settings } from "../lib/api";
import { COMMAND_NAMES, isCommandName, mockIpc, resetIpcMock, type EventMap } from "./ipc";

const settings: Settings = {
  language: "sv",
  whisper_model: "auto",
  file_transcription_model: "auto",
  pianissimo_dictation: false,
  hotkey_mode: "push",
  show_overlay: true,
  auto_paste: true,
  auto_select_model: true,
  hotkey: "Super+S",
  hotkey_profiles: [],
  profile_models: {},
  initial_prompt: "",
  profile_glossaries: {},
  beam_size: 5,
  temperature_fallback: true,
  vad_enabled: true,
  engine_prewarm: "on_key_down",
  engine_idle_unload_minutes: 10,
  has_completed_onboarding: true,
};

describe("commands", () => {
  test("records every call in order and answers with the handler's result", async () => {
    const ipc = mockIpc({
      get_settings: () => settings,
      set_language: ({ language }) => (language === "fi" ? ["Profile model reset to Auto"] : []),
    });

    expect(await getSettings()).toEqual(settings);
    expect(await setLanguage("fi")).toEqual(["Profile model reset to Auto"]);
    expect(await setLanguage("sv")).toEqual([]);

    expect(ipc.calls).toEqual([
      { command: "get_settings", args: {} },
      { command: "set_language", args: { language: "fi" } },
      { command: "set_language", args: { language: "sv" } },
    ]);
    expect(ipc.callsTo("set_language")).toEqual([{ language: "fi" }, { language: "sv" }]);
    expect(ipc.callsTo("get_platform")).toEqual([]);
  });

  test("arguments and results are copies, as across the real IPC boundary", async () => {
    mockIpc({ get_settings: () => settings });
    const first = await getSettings();
    first.language = "en";
    expect(settings.language).toBe("sv");
    expect((await getSettings()).language).toBe("sv");
  });

  test("an omitted optional argument is dropped, as JSON over the IPC boundary drops it", async () => {
    const ipc = mockIpc({ set_profile_glossary: () => undefined });
    await setProfileGlossary("default", "words");
    await setProfileGlossary("default", "more words", "words");
    expect(ipc.callsTo("set_profile_glossary")).toStrictEqual([
      { profileId: "default", source: "words" },
      { profileId: "default", source: "more words", expectedSource: "words" },
    ]);
  });

  test("reactive Svelte state is sent as its plain JSON value", async () => {
    // Components pass $state proxies, which structuredClone rejects.
    const profiles = new Proxy([{ id: "default", name: "Svenska", shortcut: "Super+S", language: "sv" as const }], {});
    expect(() => structuredClone(profiles)).toThrow();
    const ipc = mockIpc({ set_hotkey_profiles: () => [] });
    await setHotkeyProfiles(profiles);
    expect(ipc.callsTo("set_hotkey_profiles")).toStrictEqual([
      { profiles: [{ id: "default", name: "Svenska", shortcut: "Super+S", language: "sv" }] },
    ]);
  });

  test("Maps, byte buffers and IPC-serializable objects are converted as Tauri's serializer does", async () => {
    const ipc = mockIpc({ set_hotkey_profiles: () => [] });
    // Raw invoke: the typed wrappers never send these, but the boundary would.
    // They sit under the one argument `set_hotkey_profiles` reads, since the
    // mock rejects keys the Rust command does not have.
    await invoke("set_hotkey_profiles", {
      profiles: {
        labels: new Map([["s1", "Talare 1"]]),
        bytes: new Uint8Array([1, 2, 255]),
        buffer: new Uint8Array([7, 8]).buffer,
        channel: { [SERIALIZE_TO_IPC_FN]: () => "__CHANNEL__:42" },
      },
    });
    expect(ipc.calls).toStrictEqual([
      {
        command: "set_hotkey_profiles",
        args: { profiles: { labels: { s1: "Talare 1" }, bytes: [1, 2, 255], buffer: [7, 8], channel: "__CHANNEL__:42" } },
      },
    ]);
  });

  test("a handler that throws a string rejects like a Rust error", async () => {
    mockIpc({
      set_language: () => {
        throw "Profile 'default' has a personal dictionary";
      },
    });
    await expect(setLanguage("en")).rejects.toBe("Profile 'default' has a personal dictionary");
  });

  test("handle() replaces a handler between steps", async () => {
    const ipc = mockIpc({ get_platform: () => "macos" });
    expect(await getPlatform()).toBe("macos");
    ipc.handle("get_platform", async () => "windows");
    expect(await getPlatform()).toBe("windows");
    expect(ipc.calls.map((call) => call.command)).toEqual(["get_platform", "get_platform"]);
  });

  test("an unmocked command rejects and fails the test even when the caller swallows it", async () => {
    const ipc = mockIpc();
    await expect(getPlatform()).rejects.toThrow(/IPC command "get_platform" has no mock handler/);
    await getPlatform().catch(() => undefined);
    expect(ipc.unmocked).toEqual(["get_platform", "get_platform"]);
    expect(ipc.calls).toEqual([
      { command: "get_platform", args: {} },
      { command: "get_platform", args: {} },
    ]);
    // What src/test/setup.ts runs after every test:
    expect(() => resetIpcMock()).toThrow("Unmocked IPC commands were invoked: get_platform");
  });

  test("arguments the Rust command does not read reject and fail the test", async () => {
    const ipc = mockIpc({ set_language: () => [] });
    // A wrapper that drifted from the Rust signature (`language: Language`).
    await expect(invoke("set_language", { lang: "fi" })).rejects.toThrow(
      'IPC command "set_language" was invoked with arguments the Rust command does not read: missing language; no parameter lang (parameters: language)',
    );
    // Optional parameters (Rust `Option`) may be left out.
    await expect(invoke("get_file_model_options", { language: "sv" })).rejects.toThrow(/has no mock handler/);
    expect(ipc.calls).toEqual([{ command: "get_file_model_options", args: { language: "sv" } }]);
    expect(() => resetIpcMock()).toThrow("missing language; no parameter lang");
  });

  test("a new mock replaces the previous one", async () => {
    mockIpc({ get_platform: () => "macos" });
    const ipc = mockIpc({ get_platform: () => "linux" });
    expect(await getPlatform()).toBe("linux");
    expect(ipc.calls).toHaveLength(1);
  });

  test("handler arguments and results are typed by the command map", () => {
    // Compile-time only (svelte-check); never called.
    const typeChecks = () => {
      mockIpc({
        // @ts-expect-error not a command the frontend invokes
        not_a_command: () => null,
      });
      mockIpc({
        // @ts-expect-error get_platform resolves with a string
        get_platform: () => 42,
      });
      mockIpc({
        set_language: ({ language }) => {
          // @ts-expect-error set_language sends { language: Language }
          const unknownArgument: number = language;
          return [String(unknownArgument)];
        },
      });
    };
    expect(typeChecks).toBeTypeOf("function");
  });
});

describe("events", () => {
  test("emit() delivers a typed payload to listen() handlers until they unlisten", async () => {
    const ipc = mockIpc();
    const received: string[] = [];
    const unlisten = await listen<EventMap["state-changed"]>("state-changed", (event) => {
      received.push(event.payload);
    });

    expect(ipc.listenerCount("state-changed")).toBe(1);
    expect(ipc.emit("state-changed", "recording")).toBe(1);
    expect(ipc.emit("state-changed", "idle")).toBe(1);
    expect(received).toEqual(["recording", "idle"]);

    unlisten();
    await Promise.resolve();
    expect(ipc.listenerCount("state-changed")).toBe(0);
    expect(ipc.calls).toEqual([]);
  });

  test("the event object carries the name and the listener id", async () => {
    const ipc = mockIpc();
    const events: { event: string; id: number; payload: unknown }[] = [];
    await listen("update-preparing", (event) => {
      events.push(event);
    });
    ipc.emit("update-preparing", "nonce-1");
    expect(events).toEqual([{ event: "update-preparing", id: expect.any(Number), payload: "nonce-1" }]);
  });

  test("every listener of an event receives it, other events are not delivered", async () => {
    const ipc = mockIpc();
    const seen: string[] = [];
    await listen<string>("state-changed", (event) => seen.push(`a:${event.payload}`));
    await listen<string>("state-changed", (event) => seen.push(`b:${event.payload}`));
    await listen<string>("error", (event) => seen.push(`error:${event.payload}`));
    expect(ipc.emit("state-changed", "transcribing")).toBe(2);
    expect(seen).toEqual(["a:transcribing", "b:transcribing"]);
  });

  test("listeners() names every active registration, including events the map does not know", async () => {
    const ipc = mockIpc();
    expect(ipc.listeners()).toEqual([]);
    const unlistenState = await listen("state-changed", () => undefined);
    await listen("state-changed", () => undefined);
    await listen("tauri://drag-drop", () => undefined);
    await listen("error", () => undefined);
    expect(ipc.listeners()).toEqual(["error", "state-changed", "state-changed", "tauri://drag-drop"]);

    unlistenState();
    await Promise.resolve();
    expect(ipc.listeners()).toEqual(["error", "state-changed", "tauri://drag-drop"]);
  });

  test("once() receives one event and then unlistens by the event's id", async () => {
    // @tauri-apps/api's once() unlistens with `event.id` from inside the
    // handler: the id the mock delivers must be the one listen() returned.
    const ipc = mockIpc();
    const received: string[] = [];
    await once<string>("state-changed", (event) => received.push(event.payload));
    expect(ipc.emit("state-changed", "recording")).toBe(1);
    await Promise.resolve();
    expect(ipc.listenerCount("state-changed")).toBe(0);
    expect(() => ipc.emit("state-changed", "idle")).toThrow(/No listener for "state-changed"/);
    expect(received).toEqual(["recording"]);
  });

  test("emitting without a listener fails loudly", () => {
    const ipc = mockIpc();
    expect(() => ipc.emit("model-ready", null)).toThrow(/No listener for "model-ready"/);
  });

  test("events the frontend emits itself reach its listeners", async () => {
    mockIpc();
    const tabs: string[] = [];
    await listen<string>("navigate_tab", (event) => tabs.push(event.payload));
    await emit("navigate_tab", "transcribe");
    expect(tabs).toEqual(["transcribe"]);
  });

  test("event payloads are typed by the event map", () => {
    const ipc = mockIpc();
    const typeChecks = () => {
      // @ts-expect-error state-changed carries a string
      ipc.emit("state-changed", 5);
      // @ts-expect-error not an event the frontend listens for
      ipc.emit("no-such-event", null);
    };
    expect(typeChecks).toBeTypeOf("function");
  });
});

test("the command list is exhaustive and free of duplicates", () => {
  expect(new Set(COMMAND_NAMES).size).toBe(COMMAND_NAMES.length);
  expect(COMMAND_NAMES.every(isCommandName)).toBe(true);
  expect(isCommandName("plugin:dialog|open")).toBe(false);
  expect(isCommandName("toString")).toBe(false);
});
