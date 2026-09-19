<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { invoke } from '@tauri-apps/api/core';
  import { layoutStore } from '../stores/layout';

  type Approval = { run_id: string; request_id: string; command?: string; description?: string; choices: string[] };
  let pending = $state<Approval[]>([]);
  let busy = $state(false);
  let error = $state('');
  let current = $derived(pending[0]);

  onMount(() => {
    let disposed = false;
    const listeners: UnlistenFn[] = [];
    function keep(unlisten: UnlistenFn) { if (disposed) unlisten(); else listeners.push(unlisten); }
    listen<Approval>('computer-approval', ({ payload }) => {
      if (!pending.some(p => p.run_id === payload.run_id && p.request_id === payload.request_id)) {
        pending = [...pending, payload];
      }
      if (!get(layoutStore).expanded) layoutStore.expand();
    }).then(keep);
    listen<string>('computer-approval-clear', ({ payload }) => {
      pending = pending.filter(p => p.run_id !== payload);
    }).then(keep);
    listen<string>('computer-control-error', ({ payload }) => {
      error = payload;
      if (!get(layoutStore).expanded) layoutStore.expand();
    }).then(keep);
    return () => { disposed = true; listeners.forEach(fn => fn()); };
  });

  async function respond(choice: 'once' | 'deny') {
    if (!current || busy) return;
    const request = current;
    busy = true;
    error = '';
    try {
      await invoke('respond_computer_approval', { runId: request.run_id, requestId: request.request_id, choice });
      pending = pending.filter(p => p !== request);
    } catch (e) { error = String(e); }
    finally { busy = false; }
  }
</script>

{#if current || error}
  <div class="approval-backdrop" role="dialog" aria-modal="true" aria-label="操作确认">
    <div class="approval">
      {#if current}
        <strong>Hermes 请求执行操作</strong>
        <p class="details">{[current.description, current.command || '未提供操作说明'].filter(Boolean).join('\n')}</p>
      {/if}
      {#if error}<p role="alert">{error}</p>{/if}
      <div class="actions">
        {#if current}
          <button onclick={() => respond('deny')} disabled={busy}>拒绝</button>
          {#if current.choices.includes('once')}
            <button onclick={() => respond('once')} disabled={busy}>本次允许</button>
          {/if}
        {:else}
          <button onclick={() => error = ''}>我知道了</button>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .approval-backdrop { position: fixed; inset: 0; z-index: 80; background: #090b16e8; display: flex; align-items: center; justify-content: center; }
  .approval { width: 100%; max-width: 390px; max-height: 100%; overflow: auto; padding: 12px; color: #eef0ff; font-size: 12px; }
  p { margin: 7px 0; line-height: 1.4; }
  .details { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 70px; overflow: auto; user-select: text; }
  .actions { display: flex; justify-content: flex-end; gap: 8px; }
  button { padding: 5px 12px; border: 1px solid #697399; background: #252d4a; border-radius: 6px; color: #fff; cursor: pointer; }
  button:disabled { opacity: 0.5; }
</style>
