// SettingsModelDownloads (SAGA-379), driven directly: which download runs,
// what each flow does after it and where a failure shows. The rendered flows
// are in Settings.downloads.test.ts and Settings.errorRecovery.test.ts.
import { describe, expect, test, vi } from "vitest";
import { ModelDownloads, PIANISSIMO_ID } from "../models.svelte";
import { SettingsModelDownloads } from "./modelDownloads.svelte";
import { backendError, mockIpc } from "../../test/ipc";
import { deferred } from "../../test/deferred";
import { whisperModel } from "../../test/settings";

const small = whisperModel("small", "Small", { downloaded: false });
const medium = whisperModel("medium", "Medium", { downloaded: false });

// A store on a plain ModelDownloads; `events` records the callbacks in order.
function setup(download?: (id: string) => Promise<void>, refresh: () => Promise<void> = async () => {}) {
  const downloads = new ModelDownloads();
  const events: string[] = [];
  const store = new SettingsModelDownloads({
    downloads,
    onDiskChanged: () => events.push("disk"),
    refreshProfileModels: async () => {
      events.push("refresh");
      await refresh();
    },
    download,
  });
  return { downloads, events, store };
}

describe("SettingsModelDownloads state", () => {
  test("id, name, progress and busy follow the shared downloads", async () => {
    const held = deferred<void>();
    const { downloads, store } = setup(() => held.promise);

    const running = store.downloadProfileModel("default", small);
    expect(store).toMatchObject({ id: "small", name: "Small", progress: 0, busy: true });
    downloads.progress = 42;
    expect(store.progress).toBe(42);

    held.resolve();
    await running;
    expect(store).toMatchObject({ id: null, progress: 0, busy: false });
  });
});

describe("downloadProfileModel through the shared download", () => {
  test("a Pianissimo id calls download_pianissimo_model once", async () => {
    const ipc = mockIpc({ download_pianissimo_model: () => undefined });
    const { store } = setup();

    await store.downloadProfileModel("default", whisperModel(PIANISSIMO_ID, "Pianissimo", { downloaded: false }));

    expect(ipc.calls).toEqual([{ command: "download_pianissimo_model", args: {} }]);
  });

  test("another id calls download_model with that id", async () => {
    const ipc = mockIpc({ download_model: () => undefined });
    const { store } = setup();

    await store.downloadProfileModel("default", small);

    expect(ipc.calls).toEqual([{ command: "download_model", args: { whisperModel: "small" } }]);
  });
});

describe("downloadProfileModel", () => {
  test("does nothing without a model", async () => {
    const calls: string[] = [];
    const { store, events } = setup(async (id) => void calls.push(id));

    await store.downloadProfileModel("default", undefined);

    expect({ calls, events, busy: store.busy, errors: store.profileErrors }).toEqual({ calls: [], events: [], busy: false, errors: {} });
  });

  test("does nothing for a model that is already downloaded", async () => {
    const calls: string[] = [];
    const { store, events } = setup(async (id) => void calls.push(id));

    await store.downloadProfileModel("default", whisperModel("small", "Small", { downloaded: true }));

    expect({ calls, events, busy: store.busy }).toEqual({ calls: [], events: [], busy: false });
  });

  test("does nothing while another download runs", async () => {
    const held = deferred<void>();
    const calls: string[] = [];
    const { store } = setup((id) => (calls.push(id), held.promise));
    const first = store.downloadProfileModel("default", small);

    await store.downloadProfileModel("other", medium);

    expect(calls).toEqual(["small"]);
    expect(store.id).toBe("small");
    held.resolve();
    await first;
  });

  test("downloads, then reports the disk change, then refreshes, and stays busy until the refresh settled", async () => {
    const refresh = deferred<void>();
    const events: string[] = [];
    const downloads = new ModelDownloads();
    const store = new SettingsModelDownloads({
      downloads,
      onDiskChanged: () => events.push("disk"),
      refreshProfileModels: async () => {
        events.push("refresh");
        await refresh.promise;
      },
      download: async () => void events.push("download"),
    });

    const running = store.downloadProfileModel("default", small);
    await vi.waitFor(() => expect(events).toEqual(["download", "disk", "refresh"]));
    expect(events).toEqual(["download", "disk", "refresh"]);
    expect(store.busy).toBe(true);

    refresh.resolve();
    await running;
    expect(store.busy).toBe(false);
  });

  test("clears that profile's earlier message and keeps the others", async () => {
    const held = deferred<void>();
    const { store } = setup(() => held.promise);
    store.setProfileError("default", "old");
    store.setProfileError("other", "keep");

    const running = store.downloadProfileModel("default", small);

    expect(store.profileErrors).toEqual({ default: "", other: "keep" });
    held.resolve();
    await running;
  });

  test("a failed download shows the backend's message under that profile only", async () => {
    const { store, events } = setup(() => Promise.reject(backendError("download_failed", "The disk is full.")));
    store.setProfileError("other", "keep");

    await store.downloadProfileModel("default", small);

    expect(store.profileErrors).toEqual({ default: "The disk is full.", other: "keep" });
    expect(events).toEqual([]);
    expect(store.busy).toBe(false);
  });

  test("a rejection without a message shows the fallback text", async () => {
    const { store } = setup(() => Promise.reject({}));

    await store.downloadProfileModel("default", small);

    expect(store.profileErrors.default).toBe("Speech engine download failed.");
  });

  test("a refresh that rejects after the download lands under the profile", async () => {
    const { store, events } = setup(
      async () => {},
      () => Promise.reject(backendError("refresh_failed", "Could not read the models.")),
    );

    await store.downloadProfileModel("default", small);

    expect(events).toEqual(["disk", "refresh"]);
    expect(store.profileErrors.default).toBe("Could not read the models.");
    expect(store.busy).toBe(false);
  });
});

describe("downloadFileModel", () => {
  test("does nothing while another download runs", async () => {
    const held = deferred<void>();
    const calls: string[] = [];
    const { store } = setup((id) => (calls.push(id), held.promise));
    const first = store.downloadProfileModel("default", small);
    let refreshed = false;

    await store.downloadFileModel("medium", "Medium", async () => void (refreshed = true));

    expect({ calls, refreshed }).toEqual({ calls: ["small"], refreshed: false });
    held.resolve();
    await first;
  });

  test("starts under the given name, clears the message, downloads, refreshes and stays busy until that settled", async () => {
    const lineup = deferred<void>();
    const steps: string[] = [];
    const { store, events } = setup(async (id) => void steps.push(`download ${id}`));
    store.fileError = "old";

    const running = store.downloadFileModel("medium", "Medium", async () => {
      steps.push("lineup");
      await lineup.promise;
    });
    expect(store).toMatchObject({ id: "medium", name: "Medium", fileError: "", busy: true });

    await vi.waitFor(() => expect(steps.includes("lineup")).toBe(true));
    expect(steps).toEqual(["download medium", "lineup"]);
    expect(store.busy).toBe(true);

    lineup.resolve();
    await running;
    expect(store.busy).toBe(false);
    expect(events).toEqual([]);
  });

  test("a failed download ends as fileError with the store free", async () => {
    const { store, events } = setup(() => Promise.reject(backendError("download_failed", "No network.")));
    let refreshed = false;

    await store.downloadFileModel("medium", "Medium", async () => void (refreshed = true));

    expect({ error: store.fileError, busy: store.busy, refreshed, events }).toEqual({
      error: "No network.",
      busy: false,
      refreshed: false,
      events: [],
    });
  });

  test("a failed lineup refresh ends as fileError with the store free", async () => {
    const { store, events } = setup(async () => {});

    await store.downloadFileModel("medium", "Medium", () => Promise.reject(backendError("lineup_failed", "Could not read the lineup.")));

    expect({ error: store.fileError, busy: store.busy, events }).toEqual({ error: "Could not read the lineup.", busy: false, events: [] });
  });
});

describe("downloadAgain", () => {
  test("repairing outlasts busy until the profile refresh settled", async () => {
    const refresh = deferred<void>();
    const { store } = setup(
      async () => {},
      () => refresh.promise,
    );

    const running = store.downloadAgain(["small"], (id) => id);
    expect(store.repairing).toBe(true);

    await vi.waitFor(() => expect(!store.busy && store.repairing).toBe(true));
    expect(store).toMatchObject({ busy: false, repairing: true });

    refresh.resolve();
    await running;
    expect(store.repairing).toBe(false);
  });

  test("downloads one model at a time in order, reporting the disk after each and refreshing once at the end", async () => {
    const sequence: string[] = [];
    const downloads = new ModelDownloads();
    const store = new SettingsModelDownloads({
      downloads,
      onDiskChanged: () => sequence.push("disk"),
      refreshProfileModels: async () => void sequence.push("refresh"),
      download: async (id) => void sequence.push(`download ${id} as ${downloads.name}`),
    });

    await store.downloadAgain(["a", "b"], (id) => `Name ${id}`);

    expect(sequence).toEqual(["download a as Name a", "disk", "download b as Name b", "disk", "refresh"]);
  });

  test("a failing second model rejects, keeps the first one's disk change, skips the refresh and frees the store", async () => {
    const failure = backendError("download_failed", "Second failed.");
    const { store, events } = setup(async (id) => {
      if (id === "b") throw failure;
    });

    await expect(store.downloadAgain(["a", "b"], (id) => id)).rejects.toBe(failure);

    expect(events).toEqual(["disk"]);
    expect({ busy: store.busy, repairing: store.repairing }).toEqual({ busy: false, repairing: false });
    expect(store.profileErrors).toEqual({});
    expect(store.fileError).toBe("");
  });
});

describe("downloadThenSwitch", () => {
  test("downloads a model that is not on disk, then switches, staying busy until the switch settled", async () => {
    const switching = deferred<void>();
    const steps: string[] = [];
    const { store, events } = setup(async (id) => void steps.push(`download ${id}`));

    const running = store.downloadThenSwitch("default", small, async () => {
      steps.push("switch");
      await switching.promise;
    });
    await vi.waitFor(() => expect(steps.includes("switch")).toBe(true));

    expect(steps).toEqual(["download small", "switch"]);
    expect(store.busy).toBe(true);
    expect(events).toEqual([]);

    switching.resolve();
    await running;
    expect(store.busy).toBe(false);
    expect(events).toEqual(["disk"]);
  });

  test("switches a model that is already on disk without downloading", async () => {
    const calls: string[] = [];
    const { store, events } = setup(async (id) => void calls.push(id));
    let switched = false;

    await store.downloadThenSwitch("default", whisperModel("small", "Small", { downloaded: true }), async () => void (switched = true));

    expect({ calls, switched, events }).toEqual({ calls: [], switched: true, events: [] });
  });

  test("a failed download never switches and shows its message under the profile", async () => {
    const { store, events } = setup(() => Promise.reject(backendError("download_failed", "No network.")));
    let switched = false;

    await store.downloadThenSwitch("default", small, async () => void (switched = true));

    expect({ switched, errors: store.profileErrors, events, busy: store.busy }).toEqual({
      switched: false,
      errors: { default: "No network." },
      events: [],
      busy: false,
    });
  });

  test("a failed switch after the download shows its message and still reports the disk change", async () => {
    const { store, events } = setup(async () => {});

    await store.downloadThenSwitch("default", small, () => Promise.reject(backendError("switch_failed", "Could not save.")));

    expect(store.profileErrors.default).toBe("Could not save.");
    expect(events).toEqual(["disk"]);
    expect(store.busy).toBe(false);
  });

  test("a failed switch without a message falls back to the model's name", async () => {
    const { store } = setup(async () => {});

    await store.downloadThenSwitch("default", small, () => Promise.reject({}));

    expect(store.profileErrors.default).toBe("Could not switch to Small.");
  });

  test("clears the profile's earlier message when it starts", async () => {
    const held = deferred<void>();
    const { store } = setup(async () => {});
    store.setProfileError("default", "old");

    const running = store.downloadThenSwitch("default", whisperModel("small", "Small", { downloaded: true }), () => held.promise);

    expect(store.profileErrors.default).toBe("");
    held.resolve();
    await running;
  });

  test("does nothing while another download runs, and leaves that download running", async () => {
    const held = deferred<void>();
    const { store } = setup(() => held.promise);
    store.setProfileError("other", "old");
    const first = store.downloadProfileModel("default", small);
    let switched = false;

    await store.downloadThenSwitch("other", whisperModel("medium", "Medium", { downloaded: true }), async () => void (switched = true));

    expect({ switched, id: store.id, errors: store.profileErrors }).toEqual({
      switched: false,
      id: "small",
      errors: { other: "old", default: "" },
    });
    held.resolve();
    await first;
  });
});

describe("messages", () => {
  test("setProfileError sets one profile's message and keeps the others", () => {
    const { store } = setup();

    store.setProfileError("a", "first");
    store.setProfileError("b", "second");
    store.setProfileError("a", "third");

    expect(store.profileErrors).toEqual({ a: "third", b: "second" });
  });

  test("dismiss empties exactly the slots showing that message", () => {
    const { store } = setup();
    store.setProfileError("a", "same");
    store.setProfileError("b", "same");
    store.setProfileError("c", "different");
    store.fileError = "same";

    store.dismiss("same");

    expect(store.profileErrors).toEqual({ a: "", b: "", c: "different" });
    expect(store.fileError).toBe("");
  });

  test("dismiss leaves the file message alone when only a profile shows the text", () => {
    const { store } = setup();
    store.setProfileError("a", "profile text");
    store.fileError = "file text";

    store.dismiss("profile text");

    expect(store.profileErrors).toEqual({ a: "" });
    expect(store.fileError).toBe("file text");
  });
});
