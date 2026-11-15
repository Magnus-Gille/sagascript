// The tabs of the Settings window (SAGA-379), in the order of the tab strip.
// The tab strip, the tray's `navigate_tab` event and the `?tab=` of the
// window's URL all go by this list: a new panel is a new component and one
// entry here.
import type { Component } from "svelte";
import DictateTab from "./DictateTab.svelte";
import SettingsTab from "./SettingsTab.svelte";
import TranscribeTab from "./TranscribeTab.svelte";
import type { SettingsWindow } from "./settingsWindow.svelte";

/** What every panel receives. */
export interface SettingsPanelProps {
  win: SettingsWindow;
  /** Its tab is the one showing. */
  active: boolean;
}

export interface SettingsTabDefinition {
  /** What `navigate_tab`, `?tab=` and the router call the tab. */
  id: string;
  /** The text of its button in the tab strip. */
  label: string;
  panel: Component<SettingsPanelProps>;
  /**
   * Stays mounted while another tab shows, so its state and running work
   * survive a tab change; such a panel renders nothing visible while `active`
   * is false. Without it the panel is created when its tab opens and destroyed
   * when it closes.
   */
  keepMounted?: boolean;
}

export const SETTINGS_TABS: readonly SettingsTabDefinition[] = [
  { id: "dictate", label: "Dictate", panel: DictateTab },
  { id: "transcribe", label: "Transcribe", panel: TranscribeTab, keepMounted: true },
  { id: "settings", label: "Settings", panel: SettingsTab, keepMounted: true },
];
