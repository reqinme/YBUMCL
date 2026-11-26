<script lang="ts">
  /**
   * 统一的下拉：收起态是半透明胶囊，展开面板是毛玻璃。
   *
   * 用它替掉原生 <select>（WebView 里的原生下拉没法加模糊，也长得不一样）。
   * 选项值可以是任意类型，靠引用相等比较；键盘支持 ↑↓ / Home / End / Enter / Esc。
   */
  type Option = {
    value: string;
    label: string;
    /// 右侧灰色小字（Java 的 `jdk-21.0.12.1in\java.exe` 就放这里）。
    hint?: string;
    /// 悬停时给 hint 补的完整内容（完整路径）。
    hintTitle?: string;
  };

  let {
    value = $bindable(""),
    options,
    placeholder = "请选择",
    label = "",
    disabled = false,
    editable = false,
    full = false,
    onchange
  }: {
    value?: string;
    options: Option[];
    placeholder?: string;
    label?: string;
    disabled?: boolean;
    /// 可编辑模式：触发区是输入框，可以随便打字，下拉只是建议（PCL 的游戏版本就是这种）。
    editable?: boolean;
    /// 按钮上完整显示当前值，长文本换行而不是省略号。
    full?: boolean;
    onchange?: (value: string) => void;
  } = $props();

  let open = $state(false);
  let root = $state<HTMLDivElement | null>(null);
  /// 下方空间不够时把菜单翻到触发区上方（收藏夹底部的「转移到」就在窗口底边）。
  let dropUp = $state(false);
  /// 菜单最多这么高：窗口该方向上的剩余空间和 320px 取小的那个。
  let menuMax = $state(320);
  /// 菜单用 fixed 定位挂到 body 上：面板本身 overflow 裁剪，absolute 往上弹会被切掉半截。
  let menuEl = $state<HTMLUListElement | null>(null);
  let menuStyle = $state("");

  function place() {
    if (!root) return;
    const rect = root.getBoundingClientRect();
    dropUp = window.innerHeight - rect.bottom < 300;
    const space = dropUp ? rect.top - 16 : window.innerHeight - rect.bottom - 16;
    menuMax = Math.max(96, Math.min(320, space));
    const width = rect.width;
    const left = rect.left;
    if (dropUp) {
      menuStyle = `position:fixed;left:${left}px;width:${width}px;bottom:${window.innerHeight - rect.top + 6}px;max-height:${menuMax}px`;
    } else {
      menuStyle = `position:fixed;left:${left}px;width:${width}px;top:${rect.bottom + 6}px;max-height:${menuMax}px`;
    }
  }

  /// 把菜单节点挪到 body 下，避免被面板的 overflow 裁掉。
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return {
      destroy() {
        node.remove();
      }
    };
  }

  const current = $derived(options.find((option) => option.value === value) ?? null);
  /// 可编辑模式下按输入内容过滤建议项（不区分大小写）。
  const filtered = $derived(
    editable && value.trim()
      ? options.filter((option) => option.label.toLowerCase().includes(value.trim().toLowerCase()))
      : options
  );
  let inputEl = $state<HTMLInputElement | null>(null);

  function typed(event: Event) {
    value = (event.currentTarget as HTMLInputElement).value;
    place();
    open = true;
  }

  function commit() {
    open = false;
    onchange?.(value);
  }

  function toggle() {
    if (disabled) return;
    if (!open) place();
    open = !open;
  }

  function pick(option: Option) {
    if (option.value !== value) {
      value = option.value;
      onchange?.(option.value);
    }
    open = false;
  }

  function keydown(event: KeyboardEvent) {
    if (disabled) return;
    const index = filtered.findIndex((option) => option.value === value);
    if (event.key === "Escape") {
      // 只关下拉，别让这次 Esc 继续去触发页面的「返回」。
      event.stopPropagation();
      open = false;
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      if (editable) {
        commit();
        return;
      }
      open = !open;
      return;
    }
    if (event.key === " ") {
      event.preventDefault();
      open = !open;
      return;
    }
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp" && event.key !== "Home" && event.key !== "End") {
      return;
    }
    event.preventDefault();
    open = true;
    const last = filtered.length - 1;
    let next = index;
    if (event.key === "Home") next = 0;
    else if (event.key === "End") next = last;
    else if (event.key === "ArrowDown") next = index < 0 ? 0 : Math.min(last, index + 1);
    else next = index < 0 ? last : Math.max(0, index - 1);
    const target = filtered[next];
    if (target && target.value !== value) {
      value = target.value;
      onchange?.(target.value);
    }
  }

  // 点面板外面收起（菜单挂到 body 上了，也要算进来）。
  $effect(() => {
    if (!open) return;
    const away = (event: PointerEvent) => {
      const node = event.target as Node;
      if (root && !root.contains(node) && !(menuEl && menuEl.contains(node))) open = false;
    };
    const follow = () => place();
    window.addEventListener("pointerdown", away);
    window.addEventListener("scroll", follow, true);
    window.addEventListener("resize", follow);
    return () => {
      window.removeEventListener("pointerdown", away);
      window.removeEventListener("scroll", follow, true);
      window.removeEventListener("resize", follow);
    };
  });
</script>

<div class="dropdown" bind:this={root} class:open>
  {#if editable}
    <div class="trigger wrap" class:open class:full>
      <input
        class="input"
        bind:this={inputEl}
        bind:value
        {placeholder}
        {disabled}
        aria-label={label || placeholder}
        aria-haspopup="listbox"
        aria-expanded={open}
        oninput={typed}
        onfocus={() => (open = true)}
        onkeydown={keydown}
        onblur={() => setTimeout(() => (open = false), 120)}
      />
      <span class="caret" aria-hidden="true"></span>
    </div>
  {:else}
    <button
      type="button"
      class="trigger"
      class:empty={!current}
      disabled={disabled}
      aria-label={label || placeholder}
      aria-haspopup="listbox"
      aria-expanded={open}
      onclick={toggle}
      onkeydown={keydown}
    >
      <span class="value">{current?.label ?? placeholder}</span>
      <span class="caret" aria-hidden="true"></span>
    </button>
  {/if}

  {#if open}
    <ul
      class="menu"
      class:up={dropUp}
      role="listbox"
      tabindex="-1"
      bind:this={menuEl}
      use:portal
      style={menuStyle}
    >
      {#each filtered as option (option.value)}
        <li>
          <button
            type="button"
            class="item"
            class:on={option.value === value}
            role="option"
            aria-selected={option.value === value}
            onclick={() => pick(option)}
          >
            <span class="item-label">{option.label}</span>
            {#if option.hint}
              <span class="item-hint" title={option.hintTitle ?? option.hint}>{option.hint}</span>
            {/if}
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .dropdown {
    position: relative;
    min-width: 0;
  }
  .trigger {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    width: 100%;
    padding: 8px 12px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--surface);
    backdrop-filter: var(--glass-14-plus);
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
    transition:
      border-color 0.15s ease,
      background 0.15s ease;
  }
  .trigger:hover:not(:disabled) {
    background: var(--surface-2);
  }
  .dropdown.open .trigger {
    border-color: color-mix(in srgb, var(--yb-green) 55%, transparent);
  }
  .trigger:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  /* 键盘聚焦时给一圈淡绿描边，别用浏览器默认的黑框 */
  .trigger:focus-visible {
    outline: 2px solid color-mix(in srgb, var(--yb-green) 60%, transparent);
    outline-offset: 2px;
  }
  .trigger.wrap {
    cursor: text;
  }
  /* 完整显示当前值：不省略，放不下就换行 */
  .trigger.full .value {
    overflow: visible;
    text-overflow: clip;
    white-space: normal;
  }
  .input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: transparent;
    color: var(--text);
    font: inherit;
    outline: none;
  }
  .input::placeholder {
    color: var(--text-dim);
  }
  .value {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .trigger.empty .value {
    color: var(--text-dim);
  }
  .caret {
    flex: none;
    width: 7px;
    height: 7px;
    border-right: 1.5px solid currentColor;
    border-bottom: 1.5px solid currentColor;
    transform: translateY(-2px) rotate(45deg);
    opacity: 0.7;
  }
  /* 毛玻璃面板：半透明白 + 背景模糊 + 内高光边，和导航胶囊同一套材质。 */
  .menu {
    position: absolute;
    z-index: 60;
    top: calc(100% + 6px);
    left: 0;
    right: 0;
    max-height: 320px;
    overflow-y: auto;
    padding: 5px;
    margin: 0;
    list-style: none;
    border-radius: 14px;
    border: 1px solid var(--line);
    background: rgba(24, 30, 22, 0.78);
    box-shadow:
      0 18px 40px rgba(0, 0, 0, 0.42),
      inset 0 1px 0 var(--line);
    backdrop-filter: var(--glass-22);
    -webkit-backdrop-filter: var(--glass-22);
    animation: menu-in 0.14s ease;
  }
  .menu.up {
    top: auto;
    bottom: calc(100% + 6px);
    animation-name: menu-in-up;
  }
  @keyframes menu-in-up {
    from {
      opacity: 0;
      transform: translateY(4px) scaleY(0.96);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  @keyframes menu-in {
    from {
      opacity: 0;
      transform: translateY(-4px) scaleY(0.96);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  .item {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    width: 100%;
    padding: 8px 10px;
    border-radius: 9px;
    background: transparent;
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .item:hover {
    background: var(--surface-2);
  }
  .item.on {
    background: color-mix(in srgb, var(--yb-green) 26%, transparent);
  }
  .item-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .item-hint {
    flex: none;
    font-size: 11px;
    color: var(--text-dim);
  }
</style>
