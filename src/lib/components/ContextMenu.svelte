<script lang="ts">
  import { fade } from 'svelte/transition';
  import { createEventDispatcher } from 'svelte';
  import Icon from './Icon.svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { chatStore } from '../stores/chat';

  export let x = 0;
  export let y = 0;
  export let visible = false;
  export let muted = false;

  const dispatch = createEventDispatcher<{
    'toggle-mute': void;
    close: void;
  }>();

  function close() {
    visible = false;
    dispatch('close');
  }

  async function handleAction(action: string) {
    close();
    switch (action) {
      case 'clear':
        chatStore.clear();
        break;
      case 'mute':
        dispatch('toggle-mute');
        break;
      case 'quit':
        await invoke('quit_app');
        break;
    }
  }

  // 动态计算边界，防止菜单超出窗口
  $: clampedX = Math.max(0, Math.min(x, (typeof window !== 'undefined' ? window.innerWidth : 420) - 158));
  $: clampedY = Math.max(0, Math.min(y, (typeof window !== 'undefined' ? window.innerHeight : 300) - 148));
</script>

{#if visible}
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <div class="overlay" on:click={close} on:contextmenu|preventDefault={close}></div>

  <div
    class="context-menu"
    style:left="{clampedX}px"
    style:top="{clampedY}px"
    transition:fade={{ duration: 90 }}
    role="menu"
  >
    <button class="item" role="menuitem" on:click={() => handleAction('clear')}>
      <Icon name="trash-2" size={14} /> 清空对话
    </button>
    <button class="item" role="menuitem" on:click={() => handleAction('mute')}>
      {muted ? '<Icon name="bell" size={14} /> 取消静音' : '<Icon name="bell-off" size={14} /> 静音'}
    </button>
    <div class="divider"></div>
    <button class="item danger" role="menuitem" on:click={() => handleAction('quit')}>
      <Icon name="x" size={14} /> 退出
    </button>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 98;
  }

  .context-menu {
    position: fixed;
    z-index: 99;
    min-width: 150px;
    background: rgba(22, 22, 36, 0.97);
    border: 1px solid rgba(124, 158, 255, 0.22);
    border-radius: 9px;
    padding: 4px;
    /* 静态 box-shadow，不做动画，旧 GPU 安全 */
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.55);
  }

  .item {
    display: block;
    width: 100%;
    padding: 7px 12px;
    text-align: left;
    background: none;
    border: none;
    border-radius: 5px;
    color: rgba(232, 232, 240, 0.88);
    font-size: 13px;
    cursor: pointer;
    transition: background 0.1s;
  }
  .item:hover    { background: rgba(124, 158, 255, 0.14); }
  .item.danger   { color: rgba(255, 110, 110, 0.85); }
  .item.danger:hover { background: rgba(255, 80, 80, 0.12); }

  .divider {
    height: 1px;
    background: rgba(255, 255, 255, 0.07);
    margin: 3px 6px;
  }
</style>
