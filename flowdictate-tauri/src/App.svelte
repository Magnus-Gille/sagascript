<script lang="ts">
  import { onMount } from "svelte";
  import Settings from "./lib/Settings.svelte";
  import Onboarding from "./lib/Onboarding.svelte";
  import Overlay from "./lib/Overlay.svelte";

  // null = loading, true = show onboarding, false = show settings
  let showOnboarding: boolean | null = null;
  let showOverlay = false;

  onMount(async () => {
    const params = new URLSearchParams(window.location.search);

    if (params.has("overlay")) {
      document.body.style.margin = "0";
      document.body.style.background = "transparent";
      document.body.style.overflow = "hidden";
      showOverlay = true;
      return;
    }

    if (params.has("onboarding")) {
      showOnboarding = true;
      return;
    }

    // Check store for completed flag
    try {
      const { load } = await import("@tauri-apps/plugin-store");
      const store = await load("sagascript-settings.json");
      const completed = await store.get<boolean>("hasCompletedOnboarding");
      showOnboarding = !completed;
    } catch {
      showOnboarding = false;
    }
  });

  function onOnboardingComplete() {
    showOnboarding = false;
    // Strip onboarding param from URL
    const url = new URL(window.location.href);
    url.searchParams.delete("onboarding");
    window.history.replaceState({}, "", url.toString());
  }
</script>

<main>
  {#if showOverlay}
    <Overlay />
  {:else if showOnboarding === null}
    <div class="loading">
      <div class="spinner"></div>
    </div>
  {:else if showOnboarding}
    <Onboarding oncomplete={onOnboardingComplete} />
  {:else}
    <Settings />
  {/if}
</main>

<style>
  main {
    height: 100vh;
    overflow-y: auto;
  }

  .loading {
    height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--bg);
  }

  .spinner {
    width: 28px;
    height: 28px;
    border: 3px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
