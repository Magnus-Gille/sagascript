<script lang="ts">
  // Renders one panel of the Settings window on its own (SAGA-379), over the
  // window's real controllers: what Settings.svelte gives a panel, without the
  // shell around it. Use it through renderPanel (settingsPanel.ts).
  import type { Component } from "svelte";
  import { useSettingsWindow, type SettingsWindow } from "../lib/settings/settingsWindow.svelte";
  import { SETTINGS_TABS, type SettingsPanelProps } from "../lib/settings/tabs";

  let {
    panel: Panel,
    active = true,
    onWindow,
  }: {
    panel: Component<SettingsPanelProps>;
    /** The panel's tab is the one showing. */
    active?: boolean;
    /** Receives the window as the harness initializes. */
    onWindow?: (win: SettingsWindow) => void;
  } = $props();

  const win = useSettingsWindow(SETTINGS_TABS.map((tab) => tab.id));
  // The callback is read once, as the harness initializes.
  // svelte-ignore state_referenced_locally
  onWindow?.(win);
</script>

<!-- Like the window, the harness renders a panel only once the settings are loaded. -->
{#if win.session.settings}
  <Panel {win} {active} />
{/if}
