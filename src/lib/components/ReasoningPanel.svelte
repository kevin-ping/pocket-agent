<script lang="ts">
  import { chatStore } from '../stores/chat';
  import Icon from './Icon.svelte';

  let text = $derived($chatStore.reasoningText.trim());

  let contentEl = $state<HTMLDivElement>();
  let followLatest = $state(true);
  let previousLength = 0;

  function handleScroll() {
    if (!contentEl) return;
    const distanceFromBottom = contentEl.scrollHeight - contentEl.scrollTop - contentEl.clientHeight;
    followLatest = distanceFromBottom <= 8;
  }

  $effect(() => {
    const length = text.length;
    // A cleared panel starts a new turn: follow its reasoning from the top again.
    if (length < previousLength) followLatest = true;
    previousLength = length;
    if (!contentEl || !followLatest) return;

    requestAnimationFrame(() => {
      if (contentEl && followLatest) {
        contentEl.scrollTop = contentEl.scrollHeight;
      }
    });
  });
</script>

  <div class="reasoning-panel" class:visible={text !== ''}>
    <div
      class="reasoning-content"
      bind:this={contentEl}
      onscroll={handleScroll}
      aria-live="polite"
    >
      {#if text !== ''}
        <div class="reasoning-line">
          <span class="reasoning-icon" aria-hidden="true">
            <Icon name="lightbulb" size={12} color="rgba(160, 168, 255, 0.65)" />
          </span>
          <span class="reasoning-text">{text}</span>
        </div>
      {/if}
    </div>
  </div>

<style>
  .reasoning-panel {
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
  .reasoning-panel.visible {
    opacity: 1;
    visibility: visible;
    transition: opacity 0.2s ease, visibility 0s;
  }

  .reasoning-content {
    padding: 6px 10px;
    box-sizing: border-box;
    max-height: 88px; /* same cap as StatusPanel .status-content */
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-width: thin;
    scrollbar-color: rgba(160, 168, 255, 0.25) transparent;
  }
  .reasoning-content::-webkit-scrollbar       { width: 3px; }
  .reasoning-content::-webkit-scrollbar-track { background: transparent; }
  .reasoning-content::-webkit-scrollbar-thumb { background: rgba(160, 168, 255, 0.25); border-radius: 2px; }

  .reasoning-line {
    display: flex;
    align-items: flex-start;
    gap: 6px;
  }

  /* 12 px icon on the 15 px first line (10.5 px * 1.45); wrapped lines stay indented. */
  .reasoning-icon {
    display: flex;
    flex-shrink: 0;
    margin-top: 2px;
  }

  /* Dimmer than the STT text: this is Hermes' working, not what was said. */
  .reasoning-text {
    display: block;
    flex: 1;
    min-width: 0;
    font-size: 10.5px;
    line-height: 1.45;
    color: rgba(160, 168, 255, 0.65);
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
