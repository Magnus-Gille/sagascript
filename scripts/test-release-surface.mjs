import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import test from "node:test";

test("Finnish models document the generic default and pinned optional specialist", async () => {
  const [modelSources, noticeGenerator] = await Promise.all([
    readFile(new URL("../docs/model-sources.md", import.meta.url), "utf8"),
    readFile(new URL("../scripts/generate-third-party-notices.mjs", import.meta.url), "utf8"),
  ]);
  assert.match(modelSources, /Finnish/);
  assert.match(modelSources, /ggml-base\.bin/);
  assert.match(modelSources, /147,951,465/);
  assert.match(modelSources, /fi-whisper-tiny/);
  assert.match(modelSources, /77,691,730/);
  assert.match(modelSources, /41cf309b7f50523cfca724ae90924fcd0e4794205de57a66abc3cce627103ce8/);
  assert.match(noticeGenerator, /Finnish-NLP/);
  assert.match(noticeGenerator, /Apache-2\.0/);
  assert.match(noticeGenerator, /reviewed on 2026-07-10 unless a later review date is shown below/);
  for (const source of [modelSources, noticeGenerator]) {
    assert.doesNotMatch(source, /fi-whisper-medium|model-fi-medium-q5_0-b410f4a/);
  }
});

const settingsSource = await readFile(new URL("../src/lib/Settings.svelte", import.meta.url), "utf8");
const onboardingSource = await readFile(new URL("../src/lib/Onboarding.svelte", import.meta.url), "utf8");
const apiSource = await readFile(new URL("../src/lib/api.ts", import.meta.url), "utf8");
// The Rust code these assertions inspect used to be src-tauri/src/main.rs alone. #369 split it into
// modules, so the text is main.rs followed by the modules its code moved to, without their test
// files and without the three update modules (updates, activity, recovery) that already were files
// of their own. Every assertion below is unchanged except the one about the order of the single-
// instance plugin and the backend, which now names the two places where those lines live.
const appShellDirectory = new URL("../src-tauri/src/", import.meta.url);
const appShellModules = ["dictation", "launch", "settings_watcher.rs", "tray", "updater"];
const separateUpdateModules = new Set(["updater/activity.rs", "updater/recovery.rs", "updater/updates.rs"]);
async function appShellFiles(relative) {
  if (relative.endsWith(".rs")) return [relative];
  const entries = await readdir(new URL(`${relative}/`, appShellDirectory), { withFileTypes: true });
  const nested = await Promise.all(
    entries.filter((entry) => entry.isDirectory() || entry.name.endsWith(".rs")).map((entry) => appShellFiles(`${relative}/${entry.name}`)),
  );
  return nested.flat();
}
const shellPaths = (await Promise.all(appShellModules.map(appShellFiles)))
  .flat()
  .filter((path) => !/(^|\/)(\w+_)?tests\.rs$/.test(path) && !separateUpdateModules.has(path))
  .sort();
const [mainRsSource, ...moduleSources] = await Promise.all(
  ["main.rs", ...shellPaths].map((path) => readFile(new URL(path, appShellDirectory), "utf8")),
);
const mainSource = [mainRsSource, ...moduleSources].join("\n");
const startupSource = await readFile(new URL("launch/startup.rs", appShellDirectory), "utf8");
const commandsSource = await readFile(new URL("../src-tauri/src/commands.rs", import.meta.url), "utf8");
const appSource = await readFile(new URL("../src/App.svelte", import.meta.url), "utf8");
const brandMarkSource = await readFile(new URL("../assets/brand/sagascript-mark.svg", import.meta.url), "utf8");
const appIconSource = await readFile(new URL("../assets/brand/sagascript-app-icon.svg", import.meta.url), "utf8");
const faviconSource = await readFile(new URL("../site/public/favicon.svg", import.meta.url), "utf8");
const siteMarkSource = await readFile(new URL("../site/public/sagascript-mark.svg", import.meta.url), "utf8");
const siteCssSource = await readFile(new URL("../site/app/globals.css", import.meta.url), "utf8");
const sitePageSource = await readFile(new URL("../site/app/page.tsx", import.meta.url), "utf8");
const siteLayoutSource = await readFile(new URL("../site/app/layout.tsx", import.meta.url), "utf8");

test("release navigation exposes only Dictate, Transcribe, and Settings", () => {
  for (const label of ["Dictate", "Transcribe", "Settings"]) {
    assert.match(settingsSource, new RegExp(`>\\s*${label}\\s*<`));
  }
  assert.doesNotMatch(settingsSource, />\s*Teach\s*</);
  assert.doesNotMatch(settingsSource, /activeTab\s*===\s*["']teach["']/);
});

test("dictation model choice is visible while decoder controls stay behind Advanced", () => {
  const advancedStart = settingsSource.indexOf('<details class="advanced-section">');
  const advancedEnd = settingsSource.indexOf("</details>", advancedStart);
  assert.ok(advancedStart >= 0, "Advanced disclosure is missing");
  assert.ok(advancedEnd > advancedStart, "Advanced disclosure is not closed");

  const advancedSource = settingsSource.slice(advancedStart, advancedEnd);
  assert.match(advancedSource, /<summary>Advanced<\/summary>/);
  assert.doesNotMatch(advancedSource, /Manual model choice/);
  assert.match(advancedSource, /Decoding mode/);
  assert.match(advancedSource, /Temperature fallback/);
  assert.match(advancedSource, /Voice activity detection/);

  const dictateSource = settingsSource.slice(0, advancedStart);
  assert.match(dictateSource, /Speech model for \{profile\.name\}/);
  assert.match(dictateSource, /profileModelOptions\[profile\.id\]/);
});

test("ordinary settings keep the useful controls outside Advanced", () => {
  const advancedStart = settingsSource.indexOf('<details class="advanced-section">');
  const ordinarySource = settingsSource.slice(0, advancedStart);

  assert.match(ordinarySource, /Show recording overlay/);
  assert.match(ordinarySource, /Personal dictionary/);
});

test("onboarding starts with language instead of a disposable welcome step", () => {
  assert.match(onboardingSource, /currentStep:\s*Step\s*=\s*\$state\("language"\)/);
  assert.doesNotMatch(onboardingSource, /type Step = [^\n]*"welcome"/);
  assert.doesNotMatch(onboardingSource, /Welcome to Sagascript/);
});

test("cross-platform onboarding copy never identifies every device as a Mac", () => {
  const renderedMarkup = onboardingSource
    .replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, "")
    .replace(/<style\b[^>]*>[\s\S]*?<\/style>/gi, "")
    .replace(/<!--[\s\S]*?-->/g, "");
  const visibleCopy = renderedMarkup
    .replace(/\{[^{}]*\}/g, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/\s+/g, " ");

  assert.doesNotMatch(visibleCopy, /\bmac(?:os)?\b/i);
  assert.match(visibleCopy, /recommended local model is selected automatically/);
  assert.match(visibleCopy, /recordings are processed on this device/);
  assert.match(
    onboardingSource,
    /if \(platform === "macos"\) \{\s*return \["language", "download", "microphone", "accessibility", "ready"\];\s*\}\s*return \["language", "download", "ready"\];/s,
  );
});

test("onboarding automatically prepares one hidden recommended engine", () => {
  assert.match(onboardingSource, /nextStep\(\);\s*void startDownload\(\);/s);
  assert.match(onboardingSource, /local speech engine/i);
  assert.doesNotMatch(onboardingSource, /modelInfo\[selectedLanguage\]\.name/);
  assert.match(onboardingSource, /Continue without speech engine/);
  assert.match(onboardingSource, /Speech engine is not installed yet/);
});

test("dictation profiles expose missing speech engines without opening Advanced", () => {
  const advancedStart = settingsSource.indexOf('<details class="advanced-section">');
  const ordinarySource = settingsSource.slice(0, advancedStart);

  assert.match(ordinarySource, /getProfileModelInfo/);
  assert.match(ordinarySource, /Download speech engine/);
  assert.match(ordinarySource, /profileModels\[profile\.id\]\.display_name/);
  assert.match(ordinarySource, /class="link-btn profile-engine-action"/);
  assert.match(
    settingsSource,
    /\.profile-engine-action\s*\{[^}]*flex:\s*0 0 152px;[^}]*white-space:\s*nowrap;[^}]*overflow:\s*hidden;/,
    "download progress must keep a stable layout slot while its label changes",
  );
});

test("Dictate reflects backend loading and transcription instead of offering a conflicting start", () => {
  assert.match(settingsSource, /backendDictationState/);
  assert.match(settingsSource, /dictateButtonAction\(backendDictationState, testOwnsRecording\)/);
  assert.match(settingsSource, /Recording via hotkey\.\.\./);
  assert.match(settingsSource, /\["idle", "recording", "loading_model", "transcribing"\]\.includes\(nextState\)/);
  assert.match(settingsSource, /backendDictationState === "loading_model"/);
  assert.match(settingsSource, /Preparing speech engine\.\.\./);
});

test("onboarding prevents duplicate setup requests and exposes language selection", () => {
  assert.match(onboardingSource, /let platform: string \| null = \$state\(null\)/);
  assert.match(onboardingSource, /if \(languageSaving \|\| platform === null\) return;/);
  assert.match(onboardingSource, /disabled=\{languageSaving \|\| platform === null\}/);
  assert.match(onboardingSource, /platform === null \? "Preparing…"/);
  assert.equal((onboardingSource.match(/aria-pressed=\{selectedLanguage ===/g) ?? []).length, 4);
  assert.match(onboardingSource, /selectedLanguage === "fi"/);
});

test("Accessibility onboarding reopens System Settings without prompting again", () => {
  const reopenStart = onboardingSource.indexOf("async function reopenAccessibilitySettings");
  const reopenEnd = onboardingSource.indexOf("\n  }", reopenStart);

  assert.ok(reopenStart >= 0, "Accessibility reopen helper is missing");
  assert.ok(reopenEnd > reopenStart, "Accessibility reopen helper is not closed");

  const reopenSource = onboardingSource.slice(reopenStart, reopenEnd);
  assert.match(reopenSource, /await openAccessibilitySettings\(\)/);
  assert.doesNotMatch(reopenSource, /requestAccessibilityPermission|beginPollOperation|startPoll/);
  assert.match(
    apiSource,
    /export async function openAccessibilitySettings\(\): Promise<void> \{\s*return invoke\("open_accessibility_settings"\);/,
  );
  assert.match(mainSource, /commands::open_accessibility_settings,/, "Accessibility settings command is not registered with Tauri");
  assert.match(
    onboardingSource,
    /\{:else if accessibilityChecking\}[\s\S]*onclick=\{reopenAccessibilitySettings\}[\s\S]*Open Accessibility Settings Again/,
  );
});

test("tray uses compact macOS markers and a real Windows icon", () => {
  assert.match(mainSource, /TrayIconBuilder::with_id\("main"\)[\s\S]*?\.title\("S"\)/);
  assert.match(mainSource, /#\[cfg\(target_os = "windows"\)\][\s\S]*?default_window_icon\(\)[\s\S]*?tray_builder\.icon\(icon\.clone\(\)\)/);
  assert.match(mainSource, /tray\.set_visible\(true\)/);
  assert.doesNotMatch(mainSource, /include_bytes!\("\.\.\/icons\/tray-icon(?:@2x)?\.png"\)/);
  assert.doesNotMatch(mainSource, /\.icon_as_template\(true\)/);
  assert.match(mainSource, /"recording"\s*=>\s*\("Sagascript - Recording\.\.\."\s*,\s*"●"/);
  assert.match(mainSource, /"transcribing"\s*=>\s*\("Sagascript - Transcribing\.\.\."\s*,\s*"…"/);
});

test("only background startup stays headless after onboarding", () => {
  assert.match(mainSource, /GUI_BACKGROUND_ARG/);
  assert.match(mainSource, /MacosLauncher::LaunchAgent,[\s\S]*Some\(vec!\[sagascript_cli::open::GUI_BACKGROUND_ARG\]\)/);
  assert.match(mainSource, /InitialWindowRequest::Hidden/);
  assert.match(mainSource, /InitialWindowRequest::Settings/);
  assert.match(mainSource, /InitialWindowRequest::Onboarding/);
  assert.doesNotMatch(mainSource, /initial_main_window_tab/);
});

test("deliberate macOS reopen requests reveal Settings", () => {
  assert.match(
    mainSource,
    /tauri::RunEvent::Reopen \{ \.\. \} => \{[\s\S]*?open_settings_window\(_app_handle, None, "app_reopen_event"\);[\s\S]*?\}/,
  );
});

test("finishing onboarding keeps the main Settings window visible", () => {
  assert.doesNotMatch(appSource, /getCurrentWindow\(\)\.hide\(\)/);
  assert.doesNotMatch(appSource, /Failed to hide completed onboarding window/);
  assert.match(appSource, /showOnboarding = false/);
  const commandStart = commandsSource.indexOf("pub async fn set_onboarding_completed");
  const commandEnd = commandsSource.indexOf("\n#[tauri::command]", commandStart + 1);
  assert.ok(commandStart >= 0 && commandEnd > commandStart);
  const completionCommand = commandsSource.slice(commandStart, commandEnd);
  assert.doesNotMatch(completionCommand, /\.hide\(\)/);
});

test("second GUI launches are routed to the running instance", () => {
  assert.match(mainSource, /tauri_plugin_single_instance::init/);
  assert.match(mainSource, /second_instance_requests_settings/);
  assert.match(mainSource, /Second-instance launch requested Settings/);
  assert.doesNotMatch(mainSource, /Another Sagascript GUI instance is already running; exiting/);
  // The plugin is registered in main.rs before the setup hook runs; the backend is built only in
  // that hook: launch/startup.rs, where setup_app calls manage_app_state exactly once.
  const singleInstanceInit = mainRsSource.indexOf("tauri_plugin_single_instance::init");
  const setupHook = mainRsSource.indexOf(".setup(move |app| setup_app(app, gui_launch_mode))");
  assert.ok(singleInstanceInit >= 0 && setupHook > singleInstanceInit);
  assert.equal(mainSource.split("WhisperBackend::new()").length - 1, 1, "the backend is built in one place");
  assert.ok(!mainRsSource.includes("WhisperBackend::new()"), "main.rs must not build the backend outside the setup hook");
  assert.ok(!mainRsSource.includes("manage_app_state"), "main.rs must not call manage_app_state; the setup hook does");
  assert.equal(startupSource.split("manage_app_state(app);").length - 1, 1, "manage_app_state has exactly one caller");
  const setupStart = startupSource.indexOf("fn setup_app(");
  const setupLength = startupSource.slice(setupStart).search(/\r?\n\}/);
  assert.ok(setupStart >= 0 && setupLength > 0, "setup_app is not found or not closed");
  assert.ok(
    startupSource.slice(setupStart, setupStart + setupLength).includes("manage_app_state(app);"),
    "the one manage_app_state call must be inside setup_app",
  );
  assert.match(
    startupSource,
    /fn manage_app_state\(app: &tauri::App\) \{[\s\S]*?let whisper: SharedWhisper = Arc::new\(WhisperBackend::new\(\)\);[\s\S]*?app\.manage\(whisper\);/,
  );
});

test("profile and update menu states remain explicit after interaction", () => {
  assert.match(mainSource, /select_profile_menu\(app, &profile\)/);
  assert.match(mainSource, /if selected \{ "✓ " \} else \{ "" \}/);
  assert.match(mainSource, /if updates::updater_public_key\(\)\.is_some\(\) \{\s*check_for_updates_and_install\(app\.clone\(\)\)/);
  assert.match(mainSource, /state\.available_version = None;/);
  assert.match(mainSource, /items\.check\.set_text\(action_text\)/);
  assert.match(mainSource, /"Check Again…"/);
});

test("mobile site keeps navigation and readable terminal text", () => {
  assert.doesNotMatch(siteCssSource, /\.site-header nav\s*\{\s*display:\s*none/);
  assert.match(siteCssSource, /\.terminal-bar b[^}]*color:\s*#8f8f88/);
  assert.match(siteCssSource, /\.terminal code span[^}]*color:\s*#8f8f88/);
});

test("product page explains the complete privacy boundary without implying fixed profiles", () => {
  assert.match(sitePageSource, /Internet access is only used when you/);
  assert.match(sitePageSource, /check for an update/);
  assert.match(sitePageSource, /Multiple languages\. A shortcut for each\./);
  assert.doesNotMatch(sitePageSource, /Two languages\. Two shortcuts\./);
  assert.match(sitePageSource, /View source on GitHub/);
});

test("product page gives Bluetooth microphone guidance", () => {
  const bluetoothGuidance = sitePageSource
    .match(/<p className="microphone-note">([\s\S]*?)<\/p>/)?.[1]
    ?.replace(/\s+/g, " ")
    .trim();
  assert.equal(
    bluetoothGuidance,
    "Bluetooth headset microphones may give less reliable dictation, especially with fast speech. If words are missed, try a built-in microphone, if available, or a wired microphone.",
  );
});

test("app and website use the exact canonical single-S geometry", () => {
  const canonicalPath = brandMarkSource.match(/<path\s+d="([^"]+)"/)?.[1];
  assert.ok(canonicalPath, "Canonical S path is missing");

  for (const [name, source] of [
    ["app icon", appIconSource],
    ["website favicon", faviconSource],
    ["website mark", siteMarkSource],
  ]) {
    assert.ok(source.includes(`d="${canonicalPath}"`), `${name} has drifted from the canonical S geometry`);
  }
});

test("product page declares the shipped S favicon", () => {
  assert.match(siteLayoutSource, /icons:\s*\{\s*icon:\s*"\/favicon\.svg"\s*\}/);
});
