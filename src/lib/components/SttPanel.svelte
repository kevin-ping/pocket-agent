<script lang="ts">
  import { chatStore } from '../stores/chat';
  import Icon from './Icon.svelte';
</script>

  <div class="stt-panel" class:visible={$chatStore.sttText !== null}>
    <div class="stt-content" aria-live="polite">
      {#if $chatStore.sttText !== null}
        <div class="stt-line">
          <span class="stt-icon" aria-hidden="true">
            <Icon name="audio-lines" size={12} color="rgba(160, 168, 255, 0.65)" />
          </span>
          {#key $chatStore.sttText}
            <span class="stt-text">{$chatStore.sttText}</span>
          {/key}
        </div>
      {/if}
    </div>
  </div>

<style>
  .stt-panel {
    width: 100%;
    background: rgba(10, 10, 22, 0.72);
    -webkit-backdrop-filter: blur(16px) saturate(140%);
    backdrop-filter: blur(16px) saturate(140%);
    border: 1px solid rgba(160, 168, 255, 0.18);
    border-radius: 8px;
    overflow: hidden;
    /* Stays mounted; hide with opacity/visibility only. Toggling backdrop-filter on the
       transparent window leaves a stale white layer in WKWebView (same as StatusPanel). */
    opacity: 0;
    visibility: hidden;
    transition: opacity 0.2s ease, visibility 0s linear 0.2s;
  }
  .stt-panel.visible {
    opacity: 1;
    visibility: visible;
    transition: opacity 0.2s ease, visibility 0s;
  }

  .stt-content {
    padding: 6px 10px;
    box-sizing: border-box;
    max-height: 88px; /* same cap as StatusPanel .status-content */
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-width: thin;
    scrollbar-color: rgba(160, 168, 255, 0.25) transparent;
  }
  .stt-content::-webkit-scrollbar       { width: 3px; }
  .stt-content::-webkit-scrollbar-track { background: transparent; }
  .stt-content::-webkit-scrollbar-thumb { background: rgba(160, 168, 255, 0.25); border-radius: 2px; }

  .stt-line {
    display: flex;
    align-items: flex-start;
    gap: 6px;
  }

  /* 12 px icon centred on the 16 px first line (11 px * 1.45); wrapped lines stay indented. */
  .stt-icon {
    display: flex;
    flex-shrink: 0;
    margin-top: 2px;
  }

  .stt-text {
    display: block;
    flex: 1;
    min-width: 0;
    font-size: 11px;
    line-height: 1.45;
    color: rgba(232, 232, 240, 0.85);
    word-break: break-word;
    animation: fade-in 0.25s ease-out;
  }

  @keyframes fade-in {
    from { opacity: 0; transform: translateY(-3px); }
    to   { opacity: 1; transform: translateY(0); }
  }
</style>
