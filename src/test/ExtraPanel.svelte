<script lang="ts">
  // A fixture panel for Settings.tabs.test.ts: proof that a panel is one component and one entry in the tab list.
  import type { SettingsPanelProps } from "../lib/settings/tabs";

  let { win }: SettingsPanelProps = $props();
</script>

<p>An extra panel with {win.session.settings?.hotkey_profiles.length ?? 0} dictation profile(s)</p>
