<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { getActiveHotkeyProfile, getState, type HotkeyProfile } from "./api";

  let profile: HotkeyProfile | null = $state(null);
  let phase = $state("recording");
  const working = $derived(phase === "transcribing" || phase === "loading_model");
  const label = $derived.by(() => working ? "Transcribing…" : profile ? `Recording · ${languageLabel(profile.language)}` : "Recording…");

  onMount(() => {
    let disposed = false;
    let revision = 0;
    const stops: Array<() => void> = [];
    const remember = (stop: () => void) => disposed ? stop() : stops.push(stop);
    const profileListener = listen<HotkeyProfile>("active-hotkey-profile-changed", (event) => {
      revision++;
      profile = event.payload;
    }).then(remember);
    const stateListener = listen<string>("state-changed", (event) => {
      if (["recording", "transcribing", "loading_model", "idle"].includes(event.payload)) {
        revision++;
        phase = event.payload;
      }
    }).then(remember);
    Promise.all([profileListener, stateListener]).then(async () => {
      const initialRevision = revision;
      const [active, state] = await Promise.all([getActiveHotkeyProfile(), getState()]);
      if (!disposed && revision === initialRevision) { profile = active; phase = state; }
    }).catch((error) => {
      console.warn("Could not initialize dictation indicator state", error);
    });
    return () => { disposed = true; stops.forEach((stop) => stop()); };
  });

  function languageLabel(language: HotkeyProfile["language"]): string {
    return ({ en: "English", sv: "Swedish", no: "Norwegian", fi: "Finnish", pl: "Polish", auto: "Auto" })[language];
  }
</script>

<div class="pill" role="status" aria-live="polite" aria-busy={working}>
  <span class:working class="dot" aria-hidden="true"></span>
  <span class="label">{label}</span>
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

  .dot.working {
    background: transparent;
    border: 2px solid rgba(255, 255, 255, 0.3);
    border-top-color: #9cc7ff;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin { to { transform: rotate(360deg); } }

  @media (prefers-reduced-motion: reduce) {
    .dot, .dot.working { animation: none; }
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
