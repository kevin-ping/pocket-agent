<script lang="ts">
  import { invoke } from '../../ipc';

  // The phone is parked (FEAT-DEV1): PA moved to the desktop and this page
  // drives nothing. The button takes PA back; the server then sends
  // ui-parked and src/main.ts reloads into the full phone UI.
  let busy = $state(false);
  let failed = $state(false);

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
  <p class="title">PA 在电脑上</p>
  <p class="hint">要在手机上继续，点下面的按钮</p>
  <button type="button" onclick={takeOver} disabled={busy}>{busy ? '正在切回…' : '切回手机'}</button>
  {#if failed}<p class="error">没有切回来，请再试一次</p>{/if}
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
