import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const api = readFileSync(new URL("../src/lib/api.ts", import.meta.url), "utf8");
const settings = readFileSync(new URL("../src/lib/Settings.svelte", import.meta.url), "utf8");
const presenter = readFileSync(new URL("../src/lib/PresenterSettings.svelte", import.meta.url), "utf8");

test("API exposes the presenter settings contract and command", () => {
  assert.match(api, /export type HotkeyMode = "push" \| "toggle" \| "presenter";/);
  assert.match(api, /export interface PresenterConfig/);
  assert.match(api, /finish_shortcut: string;/);
  assert.match(api, /cancel_shortcut: string \| null;/);
  assert.match(api, /app_actions: Record<string, PresenterFinishAction>;/);
  assert.match(api, /invoke\("set_presenter_config", \{ config \}\)/);
});

test("Settings exposes presenter mode as an explicit conditional editor", () => {
  assert.match(settings, /<option value="presenter">Presenter<\/option>/);
  assert.match(settings, /settings\.hotkey_mode === "presenter"/);
  assert.match(settings, /<PresenterSettings/);
  assert.match(settings, /setPresenterConfig\(config\)/);
  assert.match(settings, /Presenter start shortcuts/);
});

test("Presenter editor keeps actions explicit and platform-bounded", () => {
  for (const action of ["insert_only", "return", "command_return"]) {
    assert.match(presenter, new RegExp(`value="${action}"`));
  }
  assert.match(presenter, /const supportsAutoSubmit = \(\) => platform === "macos"/);
  assert.match(presenter, /disabled=\{!supportsAutoSubmit\(\)\}/);
  assert.match(presenter, /stable IDs/);
  assert.match(presenter, /does not detect titles,?\s*sites/);
  assert.match(presenter, /at most 32 app actions/);
});

test("Presenter editor includes local canonical collision checks and preserves save errors", () => {
  assert.match(presenter, /canonicalShortcut/);
  assert.match(presenter, /Finish shortcut conflicts with a presenter start shortcut/);
  assert.match(presenter, /Cancel shortcut conflicts with a presenter start shortcut/);
  assert.match(presenter, /saveError/);
  assert.match(presenter, /onSave\(cloneConfig\(draft\)\)/);
});

test("Presenter editor uses dark-theme controls and states focused-field requirements", () => {
  assert.match(presenter, /<input\s+type="text"/);
  assert.match(presenter, /class="link-btn secondary presenter-disable-cancel"/);
  assert.match(presenter, /class="link-btn secondary" onclick=\{addAppAction\}/);
  assert.match(presenter, /requires Accessibility permission and a verifiable focused text field/);
  assert.match(presenter, /min-width: max-content/);
  assert.match(presenter, /flex-shrink: 0/);
  assert.match(presenter, /white-space: nowrap/);
});

test("Presenter application IDs stay readable in narrow action rows", () => {
  assert.match(presenter, /<code title=\{appId\}>\{appId\}<\/code>/);
  assert.match(presenter, /@media \(max-width: 520px\)/);
  assert.match(presenter, /grid-template-columns: minmax\(0, 1fr\) auto/);
  assert.match(presenter, /grid-column: 1 \/ -1/);
  assert.match(presenter, /overflow-wrap: anywhere/);
});
