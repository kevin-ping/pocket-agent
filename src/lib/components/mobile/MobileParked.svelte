<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '../../ipc';
  import { settingsStore } from '../../stores/settings';
  import { convLabelsLang } from '../../i18n';

  // The phone is parked (FEAT-DEV1): PA moved to the desktop and this page
  // drives nothing. The button takes PA back; the server then sends
  // ui-parked and src/main.ts reloads into the full phone UI.
  let busy = $state(false);
  let failed = $state(false);
  let labels = $derived(convLabelsLang($settingsStore.ui_lang));

  // The parked page doesn't mount App, so nothing else loads the settings.
  onMount(() => { settingsStore.load(); });

  async function takeOver() {
    busy = true;
    failed = false;
    try {
      await invoke('take_over');
    } catch {
      failed = true;
      busy = false;
    }
  }
</script>

<div class="parked">
  <p class="title">{labels.parkedTitle}</p>
  <p class="hint">{labels.parkedHint}</p>
  <button type="button" onclick={takeOver} disabled={busy}>{busy ? labels.parkedSwitching : labels.parkedTakeOver}</button>
  {#if failed}<p class="error">{labels.parkedFailed}</p>{/if}
</div>

<style>
  .parked {
    position: fixed;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 24px;
    background: #0b0c18;
    color: rgba(232, 232, 240, 0.92);
    font-family: system-ui, sans-serif;
    text-align: center;
  }
  .title {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
  }
  .hint {
    margin: 0 0 12px;
    font-size: 15px;
    color: rgba(160, 168, 255, 0.6);
  }
  button {
    min-height: 48px;
    padding: 0 32px;
    border-radius: 24px;
    border: 1px solid rgba(160, 168, 255, 0.4);
    background: rgba(70, 76, 180, 0.55);
    color: inherit;
    font-size: 17px;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .error {
    margin: 0;
    font-size: 14px;
    color: rgba(255, 150, 170, 0.9);
  }
</style>
