<script lang="ts">
  // Renders TabPanel for one tab definition over a real SettingsWindow (SAGA-379):
  // the window is created as SettingsPanelHarness.svelte does, with `ids` as the
  // router's tabs. Used by src/lib/settings/TabPanel.test.ts.
  import { useSettingsWindow, type SettingsWindow } from "../lib/settings/settingsWindow.svelte";
  import TabPanel from "../lib/settings/TabPanel.svelte";
  import type { SettingsTabDefinition } from "../lib/settings/tabs";

  let {
    tab,
    ids,
    onWindow,
  }: {
    tab: SettingsTabDefinition;
    /** The ids of the window's tabs; the first one is the active tab at the start. */
    ids: string[];
    /** Receives the window as the harness initializes. */
    onWindow?: (win: SettingsWindow) => void;
  } = $props();

  // The ids are read once, as the window is created.
  // svelte-ignore state_referenced_locally
  const win = useSettingsWindow(ids);
  // svelte-ignore state_referenced_locally
  onWindow?.(win);
</script>

<TabPanel {tab} {win} />
