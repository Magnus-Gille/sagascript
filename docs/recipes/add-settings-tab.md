# Recipe: add a tab to the Settings window

The Settings window is a shell (`src/lib/Settings.svelte`: header, tab strip, notices) over
panels in `src/lib/settings/`. Its tabs are one list, `SETTINGS_TABS` in
`src/lib/settings/tabs.ts`: the tab strip renders it, `TabPanel.svelte` mounts each entry,
and the router takes its ids, so the tray's `navigate_tab` event and the `?tab=` of the
window's URL accept exactly those ids. A new tab is one component and one entry.
Example: a "History" tab with the id `history`.

## 1. The panel: `src/lib/settings/HistoryTab.svelte`

```svelte
<script lang="ts">
  import type { SettingsPanelProps } from "./tabs";

  let { win, active }: SettingsPanelProps = $props();
  // The window is one object for the life of the panel.
  // svelte-ignore state_referenced_locally
  const { session } = win;

  const settings = $derived(session.settings!); // the window renders panels only once the settings are loaded
</script>

<div class="field">…</div>
```

- `win` is the `SettingsWindow` (`settingsWindow.svelte.ts`): the settings and the one way to
  save a change (`win.session`, see [add-setting.md](add-setting.md)), the router
  (`win.router`), and the other controllers. Import it as a type only.
- State that only this panel needs lives in the panel. State that must outlive the panel or
  that another part of the window reads goes in a controller class in a `.svelte.ts` file,
  created in `SettingsWindow`'s constructor (see `fileQueue.svelte.ts`).
- The root elements of a panel are direct children of the shell's `.content`. The shell's
  scoped CSS does not reach them, so a panel's `<style>` starts with the three rules every
  panel repeats: copy them from the top of `AdvancedPanel.svelte`'s `<style>` and keep the
  element kinds your markup has.
- A settings change that fails is reported in the tab's own error,
  `win.errors.of("history")` (`tabErrors.svelte.ts`): pass it to `session.apply` or
  `session.applyScalar`, which clear it when the change starts and write the message when
  the change is rejected, or set its `message`. The shell shows it above the panel while the
  tab shows and marks the tab's button while another tab shows; nothing on another tab shows
  or clears it. A message that belongs next to one control stays in the panel instead.
- A panel that asks before the tab changes (unsaved work) sets `win.router.guard`, as
  `SettingsTab.svelte` does for the dictionary editor. A panel that must hold the tabs while
  something is in flight sets `win.router.held`.

## 2. The entry: `src/lib/settings/tabs.ts`

```ts
import HistoryTab from "./HistoryTab.svelte";

export const SETTINGS_TABS: readonly SettingsTabDefinition[] = [
  …
  { id: "history", label: "History", panel: HistoryTab },
];
```

The position in the list is the position in the tab strip; the first entry is the tab the
window opens on. Without `keepMounted` the panel is created when its tab opens and destroyed
when it closes, like Dictate. With `keepMounted: true` it stays mounted and gets
`active = false` while another tab shows: use it when the panel's state or running work
must survive a tab change, and render nothing visible while `active` is false (Transcribe
hides its section with `hidden`; Settings renders its controls inside `{#if active}`).

Nothing else in the frontend lists the tabs; a panel names its own id only for its error.

## 3. Opening it from the backend (only if needed)

`open_settings_window(app, Some("history"), reason)` in `src-tauri/src/launch/window.rs`
opens the window on a tab: it loads `index.html?tab=history` for a new window and emits
`navigate_tab` to an open one. The tray menu calls it in `src-tauri/src/tray/mod.rs`. An id
that is not in `SETTINGS_TABS` is ignored by the window.

## 4. Tests

| Where | What |
|---|---|
| `src/lib/settings/HistoryTab.test.ts` | The panel on its own: `renderPanel(HistoryTab, handlers, { active })` from `src/test/settingsPanel.ts` ([docs/testing/frontend.md](../testing/frontend.md)). |
| `src/lib/settings/tabs.test.ts` | Add the id and the label to the expected order. |
| `src/lib/Settings.tabs.test.ts` | Add the label to the expected tab strip and the tab to the `navigate_tab` test. |
| `src/lib/Settings.tabErrors.test.ts` | Only if the panel reports errors: one that fails shows on its tab and marks its button while another tab shows. |
| `scripts/fixtures/transcription-browser-mock.js` | An answer for every new command the panel sends while it mounts ([add-tauri-command.md](add-tauri-command.md)). |

## 5. Verify

```bash
npm run verify:fast
npx playwright install chromium && npm run test:browser
```
