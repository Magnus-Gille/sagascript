<script lang="ts">
  // A fixture panel for TabPanel.test.ts: some text and the `active` value the
  // panel was given, so a test can see what TabPanel tells its panel.
  import type { SettingsPanelProps } from "../lib/settings/tabs";

  let { active }: SettingsPanelProps = $props();
</script>

<p>Fixture panel content</p>
<p>active is {active}</p>
