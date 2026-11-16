// The Settings window's composition root (SAGA-379): its controllers, the
// backend events it listens to and what it loads when it mounts. Each
// controller reaches its siblings through this object.
import { onMount } from "svelte";
import {
  acknowledgeUpdateResult,
  checkAccessibilityPermission,
  clearUpdateRecovery,
  completeUpdatePreparation,
  getBuildInfo,
  getEngineStatus,
  getLastTranscription,
  getSupportedFormats,
  getUpdateResultPending,
  hotkeyStatus,
  loadUpdateRecovery,
  saveUpdateRecovery,
  type BuildInfo,
  type EngineStatus,
} from "../api";
import { toErrorMessage } from "../errors";
import { loadPlatform } from "../platform";
import { useDragDropEvent, useTauriEvent } from "../tauri-events.svelte";
import { Dictation } from "./dictation.svelte";
import { FileModel } from "./fileModel.svelte";
import { FileQueue } from "./fileQueue.svelte";
import { HotkeyProfiles } from "./hotkeyProfiles.svelte";
import { useSettingsModelDownloads, type SettingsModelDownloads } from "./modelDownloads.svelte";
import { ProfileModels } from "./profileModels.svelte";
import { SettingsSession } from "./settingsSession.svelte";
import { TabErrors } from "./tabErrors.svelte";
import { TabRouter } from "./tabRouter.svelte";
import { UpdateRecoveryController } from "./updateRecovery.svelte";

/** The tab the window was opened on (`index.html?tab=`), read once when it mounts. */
function tabFromUrl(): string | null {
  const params = new URLSearchParams(window.location.search);
  return params.get("tab");
}

export class SettingsWindow {
  platform: string = $state("unknown");
  buildInfo: BuildInfo | null = $state(null);
  engineStatus = $state<EngineStatus | null>(null);
  engineHostIdentity = $derived(
    this.engineStatus?.host_version
      ? `Engine ${this.engineStatus.host_version}${this.engineStatus.host_git_sha ? ` (${this.engineStatus.host_git_sha.slice(0, 7)})` : ""}`
      : "",
  );

  readonly session: SettingsSession;
  readonly router: TabRouter;
  /** The error of each tab: a change that fails is reported on the tab it was made on. */
  readonly errors: TabErrors;
  readonly downloads: SettingsModelDownloads;
  readonly profileModels: ProfileModels;
  readonly profiles: HotkeyProfiles;
  readonly dictation: Dictation;
  readonly queue: FileQueue;
  readonly fileModel: FileModel;
  readonly recovery: UpdateRecoveryController;

  get updatePreparing(): boolean {
    return this.recovery.preparing;
  }

  constructor(tabIds: readonly string[]) {
    // One download at a time, with the errors it leaves (settings/modelDownloads.svelte.ts);
    // its progress follows the backend's events.
    this.downloads = useSettingsModelDownloads({
      onDiskChanged: () => {
        this.fileModel.diskRevision += 1;
      },
      refreshProfileModels: () => this.profileModels.refresh(this.session.settings?.hotkey_profiles ?? []),
    });

    this.session = new SettingsSession(this);
    this.router = new TabRouter(tabIds);
    this.errors = new TabErrors(tabIds);
    this.profileModels = new ProfileModels(this);
    this.profiles = new HotkeyProfiles(this);
    this.dictation = new Dictation(this);
    this.queue = new FileQueue(this);
    this.fileModel = new FileModel(this);

    // Update recovery (settings/updateRecovery.svelte.ts) reads and writes the
    // dictation editor and the Transcribe tab's file queue through these two ports.
    this.recovery = new UpdateRecoveryController({
      api: {
        loadUpdateRecovery,
        saveUpdateRecovery,
        clearUpdateRecovery,
        completeUpdatePreparation,
        getLastTranscription,
        getUpdateResultPending,
        acknowledgeUpdateResult,
      },
      dictation: this.dictation,
      files: this.queue,
    });

    const errorEvents = useTauriEvent("error", (failure) => this.dictation.onError(failure));
    const resultEvents = useTauriEvent("transcription-result", (text) => this.dictation.onResult(text));

    onMount(() => {
      let disposed = false;
      // A failed background dictation may create this window after its event.
      // Recover the persisted-in-memory result without racing newer events.
      Promise.all([errorEvents.ready, resultEvents.ready, stateEvents.ready])
        .then(() => this.dictation.restoreLast(() => disposed))
        .catch((error) => {
          console.warn("Could not restore the last dictation result", error);
        });
      return () => {
        disposed = true;
      };
    });

    $effect(() => {
      this.queue.startNextQueued();
    });

    $effect(() => this.fileModel.requestLineup());

    $effect(() => {
      this.queue.reconcileProfile();
    });

    // The listeners below register as the component initializes, before the
    // data fetch in onMount: they don't depend on it, so a rejected invoke in
    // the fetch sequence must never prevent them from wiring up (e.g. a
    // stuck-at-0% download).
    useTauriEvent("update-preparing", (nonce) => {
      if (nonce) this.recovery.beginPreparation(nonce);
    });
    useTauriEvent("update-aborted", () => this.recovery.abortPreparation());

    useTauriEvent("model-ready", async () => {
      // The event can arrive before the download command settles. Its caller
      // owns the busy flag through its finally block so a second operation
      // cannot start during the remaining selection or refresh work.
      if (this.session.settings) await this.profileModels.refresh(this.session.settings.hotkey_profiles);
    });

    // Hotkey registration health can change at any time (settings-file
    // hot-reload, a failed re-register racing a Spotlight/Raycast combo
    // claim, etc.) — not just as a result of something this window did.
    useTauriEvent("hotkey-registration-changed", (status) => this.profiles.setStatus(status));

    // Keep Dictate synchronized with hotkey-driven work. Previously these
    // states were ignored, so the button still offered "Start recording"
    // while the backend was loading/transcribing and the next click produced
    // a misleading busy error.
    const stateEvents = useTauriEvent("state-changed", (nextState) => this.dictation.onStateChanged(nextState));

    // Listen for tab navigation from tray menu
    useTauriEvent("navigate_tab", (t) => {
      if (this.router.has(t)) this.router.request(t);
    });

    // Listen for drag-and-drop events
    useDragDropEvent((payload) => this.queue.onDragDrop(payload));

    onMount(() => {
      this.recovery.restore();

      // Fetch initial data. A rejection here surfaces as a visible inline
      // error instead of leaving a half-initialized window.
      (async () => {
        this.session.initError = "";
        try {
          this.buildInfo = await getBuildInfo();
        } catch (error) {
          // Build identity is diagnostic only. Keep it independent from the
          // settings bootstrap so it remains visible when another query fails.
          console.warn("Failed to load build information", error);
        }
        getEngineStatus()
          .then((status) => {
            this.engineStatus = status;
          })
          .catch((error) => {
            // Diagnostic and optional: Pianissimo controls stay hidden without it.
            console.warn("Failed to load engine status", error);
          });
        try {
          const loaded = await this.session.read();
          await this.profileModels.refresh(loaded.hotkey_profiles);
          this.platform = await loadPlatform();
          if (this.platform === "macos") {
            this.profiles.accessibilityGranted = await checkAccessibilityPermission();
          }
          this.queue.supportedFormats = await getSupportedFormats();
          const status = await hotkeyStatus();
          this.profiles.statusOk = status.ok;
          this.profiles.statusError = status.error ?? "";
          if (
            this.platform === "macos" &&
            this.profiles.accessibilityGranted &&
            !status.ok &&
            this.profiles.configuredShortcutsUseBareHotkey()
          ) {
            await this.profiles.refreshRegistration();
          }

          // Check URL params for initial tab
          const tab = tabFromUrl();
          if (tab !== null && this.router.has(tab)) {
            this.router.request(tab);
          }
        } catch (e) {
          this.session.initError = toErrorMessage(e, "Failed to load settings.");
        }
      })();
      return () => {
        this.recovery.dispose();
        this.fileModel.dispose();
        this.profileModels.dispose();
      };
    });
  }
}

/** The Settings window's controllers. Call it while a component initializes: it registers effects, listeners and mount callbacks. */
export function useSettingsWindow(tabIds: readonly string[]): SettingsWindow {
  return new SettingsWindow(tabIds);
}
