<script lang="ts">
  let { title, phase, points }: { title: string; phase: string; points: string[] } = $props();
</script>

<section class="placeholder">
  <span class="phase">{phase}</span>
  <h1>{title}</h1>
  <ul>
    {#each points as point}
      <li>{point}</li>
    {/each}
  </ul>
</section>

<style>
  .placeholder {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .phase {
    align-self: flex-start;
    font-size: 11px;
    letter-spacing: 0.08em;
    padding: 3px 9px;
    border-radius: 999px;
    background: rgba(127, 163, 94, 0.16);
    border: 1px solid rgba(127, 163, 94, 0.34);
    color: var(--yb-green-bright);
  }
  h1 {
    margin: 4px 0 0;
    font-size: 22px;
    font-weight: 650;
  }
  ul {
    margin: 0;
    padding-left: 18px;
    color: rgba(255, 255, 255, 0.72);
    line-height: 1.9;
  }
</style>
