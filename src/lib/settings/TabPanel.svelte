<script lang="ts">
  // One entry of SETTINGS_TABS in the window's content area (SAGA-379): a panel
  // that keeps its state is always mounted and told whether it shows; any other
  // exists only while its tab is the active one.
  import type { SettingsWindow } from "./settingsWindow.svelte";
  import type { SettingsTabDefinition } from "./tabs";

  let { tab, win }: { tab: SettingsTabDefinition; win: SettingsWindow } = $props();
  const active = $derived(win.router.active === tab.id);
</script>

{#if tab.keepMounted || active}
  <tab.panel {win} {active} />
{/if}
