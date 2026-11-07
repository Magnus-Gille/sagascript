<script lang="ts">
  import { onMount } from "svelte";
  import { getActiveHotkeyProfile, getEngineLoadState, getState, type HotkeyProfile } from "./api";
  import { useTauriEvent } from "./tauri-events.svelte";
  import {
    LOADING_LABEL_DELAY_MS,
    initialOverlayState,
    isWorking,
    needsLoadingTimer,
    overlayLabel,
    reduceOverlay,
    type OverlayAction,
  } from "./overlay-state";

  let profile = $state<HotkeyProfile | null>(null);
  let overlay = $state(initialOverlayState);
  const working = $derived(isWorking(overlay.phase));
  const failed = $derived(overlay.phase === "error");
  const label = $derived(overlayLabel(overlay, profile?.language ?? null, navigator.language));

  function dispatch(action: OverlayAction) {
    overlay = reduceOverlay(overlay, action);
  }

  // Show "Loading model" only after the load has kept the user waiting for a
  // moment, so a warm engine never flashes the label.
  $effect(() => {
    if (!needsLoadingTimer(overlay)) return;
    const timer = setTimeout(() => dispatch({ type: "loading-delay-elapsed" }), LOADING_LABEL_DELAY_MS);
    return () => clearTimeout(timer);
  });

  let revision = 0;
  const profileEvents = useTauriEvent("active-hotkey-profile-changed", (payload) => {
    revision++;
    profile = payload;
  });
  const stateEvents = useTauriEvent("state-changed", (value) => {
    revision++;
    dispatch({ type: "state", value });
  });
  const engineEvents = useTauriEvent("engine-load-state", (payload) => {
    revision++;
    dispatch({ type: "engine", value: payload.state, source: payload.source });
  });

  onMount(() => {
    let disposed = false;
    Promise.all([profileEvents.ready, stateEvents.ready, engineEvents.ready])
      .then(async () => {
        const initialRevision = revision;
        const [active, state, engineLoad] = await Promise.all([getActiveHotkeyProfile(), getState(), getEngineLoadState()]);
        if (!disposed && revision === initialRevision) {
          profile = active;
          dispatch({ type: "state", value: state });
          // The overlay is created lazily, after the app-start warm began.
          dispatch({ type: "engine", value: engineLoad.state, source: engineLoad.source });
        }
      })
      .catch((error) => {
        console.warn("Could not initialize dictation indicator state", error);
      });
    return () => {
      disposed = true;
    };
  });
</script>

<div class="pill" role="status" aria-live="polite" aria-busy={working}>
  <span class:working class:failed class="dot" aria-hidden="true"></span>
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

  .dot.failed {
    background: #ff9f0a;
    animation: none;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .dot,
    .dot.working {
      animation: none;
    }
  }

  @keyframes pulse {
    0%,
    100% {
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
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    white-space: nowrap;
    max-width: 160px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
