<script lang="ts">
  /// 滑动开关：卡片标题右边的「放进便捷功能」。
  let {
    checked,
    label,
    onchange,
  }: { checked: boolean; label: string; onchange: (value: boolean) => void } =
    $props();
</script>

<button
  class="switch"
  class:on={checked}
  role="switch"
  aria-checked={checked}
  aria-label={label}
  title={label}
  onclick={() => onchange(!checked)}
>
  <span class="knob"></span>
</button>

<style>
  .switch {
    position: relative;
    flex: none;
    width: 34px;
    height: 18px;
    padding: 0;
    border-radius: 999px;
    border: 1px solid var(--tint-4);
    background: var(--tint-2);
    cursor: pointer;
    transition: background 0.18s ease, border-color 0.18s ease;
  }

  .switch:focus-visible {
    outline: 2px solid var(--yb-green-bright);
    outline-offset: 2px;
  }

  .switch.on {
    background: rgb(120 200 120 / 0.45);
    border-color: rgb(140 220 140 / 0.55);
  }

  .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: #e9f2ea;
    transition: transform 0.18s ease;
  }

  .switch.on .knob {
    transform: translateX(16px);
    background: #ffffff;
  }
</style>
