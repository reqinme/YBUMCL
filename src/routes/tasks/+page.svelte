<script lang="ts">
  // 下载任务页：导航栏那颗圆钮点进来就是这里，占满内容区。
  import TasksPanel from "$lib/components/TasksPanel.svelte";
  import { goBackPage } from "$lib/page-history.svelte";
</script>

<TasksPanel onback={() => void goBackPage()} />
