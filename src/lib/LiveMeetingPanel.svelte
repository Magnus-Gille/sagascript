<script lang="ts">
  import { onMount } from "svelte";
  import LiveMeeting from "./LiveMeeting.svelte";
  import { toErrorMessage } from "./errors";
  import type { LiveSnapshot } from "./live-meeting-types";

  export interface LiveApi {
    load: () => Promise<LiveSnapshot | null>;
    start: () => Promise<void>;
    stop: (id: string) => Promise<void>;
    discard: (id: string) => Promise<void>;
    saveText: (id: string) => Promise<void>;
    saveAudio: (id: string) => Promise<void>;
  }

  interface Props {
    api: LiveApi;
    supported: boolean;
  }

  let { api, supported }: Props = $props();

  let snapshot = $state<LiveSnapshot | null>(null);
  let initialLoadDone = $state(false);
  let loadError = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let actionBusy = $state<string | null>(null);
  let mounted = false;
  let generation = 0;
  let loadPromise: Promise<void> | null = null;

  function errorMessage(error: unknown): string {
    return toErrorMessage(error, "The live meeting action failed.");
  }

  function loadSnapshot(allowDuringAction = false): Promise<void> {
    if (!mounted || (!allowDuringAction && actionBusy !== null)) return Promise.resolve();
    if (loadPromise) return loadPromise;
    const requestGeneration = generation;
    let loadResult: Promise<LiveSnapshot | null>;
    try {
      loadResult = api.load();
    } catch (error: unknown) {
      loadResult = Promise.reject(error);
    }
    const request = loadResult
      .then((nextSnapshot) => {
        if (!mounted || requestGeneration !== generation) return;
        snapshot = nextSnapshot;
        loadError = null;
        initialLoadDone = true;
      })
      .catch((error: unknown) => {
        if (!mounted || requestGeneration !== generation) return;
        loadError = errorMessage(error);
        initialLoadDone = true;
      })
      .finally(() => {
        if (loadPromise === request) loadPromise = null;
      });
    loadPromise = request;
    return request;
  }

  async function reloadAfterAction(): Promise<void> {
    const currentLoad = loadPromise;
    if (currentLoad) await currentLoad;
    if (mounted) await loadSnapshot(true);
  }

  async function runAction(name: string, action: () => Promise<void>, reload = true): Promise<void> {
    if (!mounted || actionBusy !== null) return;
    actionBusy = name;
    actionError = null;
    // Any load already in flight belongs to the pre-action view. Its result
    // must not overwrite the action's eventual reconciliation load.
    generation += 1;
    let failure: unknown = null;
    try {
      await action();
    } catch (error: unknown) {
      failure = error;
    }
    if (reload && mounted) await reloadAfterAction();
    if (failure !== null && mounted) actionError = errorMessage(failure);
    if (mounted) actionBusy = null;
  }

  function start(): Promise<void> {
    return runAction("start", api.start);
  }

  function stop(): Promise<void> {
    const id = snapshot?.id;
    return id ? runAction("stop", () => api.stop(id)) : Promise.resolve();
  }

  function discard(): Promise<void> {
    const id = snapshot?.id;
    return id ? runAction("discard", () => api.discard(id)) : Promise.resolve();
  }

  function saveText(): Promise<void> {
    const id = snapshot?.id;
    return id ? runAction("save-text", () => api.saveText(id), false) : Promise.resolve();
  }

  function saveAudio(): Promise<void> {
    const id = snapshot?.id;
    return id ? runAction("save-audio", () => api.saveAudio(id), false) : Promise.resolve();
  }

  async function refreshSnapshot(): Promise<void> {
    if (!mounted) return;
    generation += 1;
    const currentLoad = loadPromise;
    if (currentLoad) await currentLoad;
    if (mounted) await loadSnapshot(true);
  }

  function refresh(): Promise<void> {
    return runAction("refresh", refreshSnapshot, false);
  }

  onMount(() => {
    mounted = true;
    void loadSnapshot();
    const poller = setInterval(() => {
      void loadSnapshot();
    }, 500);
    return () => {
      mounted = false;
      generation += 1;
      clearInterval(poller);
    };
  });

  const panelBusy = $derived(actionBusy !== null || !initialLoadDone);
  const panelError = $derived(actionError ?? loadError);
</script>

<LiveMeeting
  {snapshot}
  busy={panelBusy}
  {supported}
  onStart={start}
  onStop={stop}
  onDiscard={discard}
  onSaveText={saveText}
  onSaveAudio={saveAudio}
  onRefresh={refresh}
  error={panelError}
/>
