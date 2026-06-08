<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { settingsStore } from '../stores/settings';
  import { tLang } from '../i18n';

  let { visible = false, onbreak, oncancel }: {
    visible?: boolean;
    onbreak: () => void;
    oncancel: () => void;
  } = $props();

  const strings = $derived(tLang(get(settingsStore).ui_lang ?? ''));

  function handleKey(e: KeyboardEvent) {
    if (!visible) return;
    if (e.key === 'Enter') { e.preventDefault(); onbreak(); }
    else if (e.key === 'Escape') { e.preventDefault(); oncancel(); }
  }

  onMount(() => window.addEventListener('keydown', handleKey));
  onDestroy(() => window.removeEventListener('keydown', handleKey));
</script>

{#if visible}
  <div class="modal-backdrop" role="dialog" aria-modal="true">
    <div class="modal">
      <p class="modal-title">{strings.breakConfirmTitle}</p>
      <div class="modal-actions">
        <button class="btn" onclick={oncancel}>{strings.breakConfirmContinue}</button>
        <!-- svelte-ignore a11y_autofocus -->
        <button class="btn primary" autofocus onclick={onbreak}>{strings.breakConfirmBreak}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed; inset: 0; z-index: 60;
    background: rgba(0, 0, 0, 0.55);
    display: flex; align-items: center; justify-content: center;
    -webkit-backdrop-filter: blur(4px); backdrop-filter: blur(4px);
  }
  .modal {
    min-width: 240px; max-width: 320px;
    background: rgba(14, 14, 26, 0.98);
    border: 1px solid rgba(160, 168, 255, 0.28);
    border-radius: 12px;
    padding: 14px 16px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.7);
  }
  .modal-title {
    margin: 0 0 12px 0;
    font-size: 12.5px;
    line-height: 1.45;
    color: rgba(232, 234, 255, 0.92);
  }
  .modal-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  .btn {
    padding: 6px 14px;
    font-size: 12px;
    border-radius: 8px;
    cursor: pointer;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid rgba(255, 255, 255, 0.13);
    color: rgba(232, 234, 255, 0.92);
  }
  .btn.primary {
    background: rgba(124, 158, 255, 0.22);
    border-color: rgba(124, 158, 255, 0.42);
  }
  .btn:hover { filter: brightness(1.12); }
</style>
