<!-- A component that listens through the lifecycle helpers, for testing them. Never bundled. -->
<script lang="ts">
  import { useDragDropEvent, useTauriEvent, type Subscription } from "../lib/tauri-events.svelte";
  import type { DragDropEvent } from "@tauri-apps/api/webview";

  let {
    onState,
    onDrop,
    onSubscribed,
  }: {
    onState: (payload: string) => void;
    onDrop?: (payload: DragDropEvent) => void;
    onSubscribed?: (subscriptions: Subscription[]) => void;
  } = $props();

  const subscriptions = [useTauriEvent("state-changed", (payload) => onState(payload))];
  // svelte-ignore state_referenced_locally
  if (onDrop) subscriptions.push(useDragDropEvent((payload) => onDrop(payload)));
  // svelte-ignore state_referenced_locally
  onSubscribed?.(subscriptions);
</script>
