<script lang="ts">
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { downloadsNav } from "$lib/downloads-nav.svelte";

  // 兼容直接访问 /tasks：把下载页的任务层打开，再回到下载页（不再单独渲染一份任务列表）。
  onMount(() => {
    downloadsNav.tasksOpen = true;
    void goto("/downloads", { replaceState: true });
  });
</script>

<div class="page"></div>
