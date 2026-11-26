<script lang="ts">
  import { launchProgress } from "$lib/launch-pending.svelte";

  let { id }: { id: string } = $props();
  const value = $derived(launchProgress(id));
</script>

{#if value !== null}
  <span class="launch-fill" style="width: {value}%" aria-hidden="true"></span>
{/if}

<style>
  .launch-fill {
    position: absolute;
    inset: 0 auto 0 0;
    background: linear-gradient(
      90deg,
      var(--tint-2),
      var(--glint)
    );
    border-right: 2px solid var(--text-2);
    box-shadow: 0 0 12px var(--line-strong);
    transition: width 0.3s ease-out;
    pointer-events: none;
  }
</style>
