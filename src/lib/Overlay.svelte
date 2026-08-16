<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { getActiveHotkeyProfile, type HotkeyProfile } from "./api";

  let profile: HotkeyProfile | null = $state(null);

  onMount(() => {
    let unlisten = () => {};
    getActiveHotkeyProfile().then((active) => { profile = active; });
    listen<HotkeyProfile>("active-hotkey-profile-changed", (event) => {
      profile = event.payload;
    }).then((stop) => { unlisten = stop; });
    return () => unlisten();
  });

  function languageLabel(language: HotkeyProfile["language"]): string {
    return ({ en: "English", sv: "Swedish", no: "Norwegian", auto: "Auto" })[language];
  }
</script>

<div class="pill">
  <span class="dot"></span>
  <span class="label">{profile ? `${profile.name} · ${languageLabel(profile.language)}` : "Recording..."}</span>
</div>

<style>
  .pill {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 20px;
    background: rgba(30, 30, 30, 0.85);
    backdrop-filter: blur(12px);
    -webkit-backdrop-filter: blur(12px);
    border-radius: 24px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.3);
    user-select: none;
  }

  .dot {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: #ff3b30;
    animation: pulse 1.5s ease-in-out infinite;
    flex-shrink: 0;
  }

  @keyframes pulse {
    0%, 100% {
      opacity: 1;
      transform: scale(1);
    }
    50% {
      opacity: 0.5;
      transform: scale(1.3);
    }
  }

  .label {
    color: #fff;
    font-size: 14px;
    font-weight: 500;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
    white-space: nowrap;
    max-width: 160px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
