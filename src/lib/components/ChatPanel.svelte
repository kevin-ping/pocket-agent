<script lang="ts">
  import { fly } from 'svelte/transition';
  import { chatStore } from '../stores/chat';
  import { settingsStore } from '../stores/settings';
  import { t } from '../i18n';
  import { characterState } from '../stores/character';

  let { side = 'right', onSend, onCollapse }: { side?: 'left' | 'right'; onSend: (text: string) => void; onCollapse: () => void } = $props();

  let inputText = $state('');
  let contentEl: HTMLDivElement | undefined;
  let panelEl: HTMLDivElement | undefined;

  // Sanitize HTML: only allow safe tags and attributes
  function sanitizeHtml(html: string): string {
    const div = document.createElement('div');
    div.innerHTML = html;
    
    // Remove all script tags and event handlers
    const scripts = div.querySelectorAll('script');
    scripts.forEach(script => script.remove());
    
    // Remove event handlers from all elements
    const allElements = div.querySelectorAll('*');
    allElements.forEach(el => {
      // Remove all on* attributes
      Array.from(el.attributes).forEach(attr => {
        if (attr.name.startsWith('on')) {
          el.removeAttribute(attr.name);
        }
      });
      
      // Only allow specific tags
      const allowedTags = ['A', 'BR', 'P', 'SPAN', 'STRONG', 'EM', 'B', 'I', 'U', 'CODE', 'PRE'];
      if (!allowedTags.includes(el.tagName)) {
        // Replace with text content
        const text = document.createTextNode(el.textContent || '');
        el.parentNode?.replaceChild(text, el);
      }
    });
    
    // Only allow href attribute on links
    const links = div.querySelectorAll('a');
    links.forEach(link => {
      const href = link.getAttribute('href');
      const linkText = link.textContent || href || '';
      Array.from(link.attributes).forEach(attr => link.removeAttribute(attr.name));
      if (href && (href.startsWith('http://') || href.startsWith('https://') || href.startsWith('mailto:'))) {
        link.setAttribute('href', href);
        link.setAttribute('target', '_blank');
        link.setAttribute('rel', 'noopener noreferrer');
      }
            link.textContent = linkText;
    });
    
    return div.innerHTML;
  }

  // During thinking/tool phase, show intermediate steps instead of empty box
  let displayContent = $derived($chatStore.isStreaming && !$chatStore.streamingContent
    ? ''
    : ($chatStore.error ?? $chatStore.messages.at(-1)?.content ?? t($settingsStore.tts_primary_voice).hint.replace("{key}", $settingsStore.hotkey_name)));

  // Auto-scroll to bottom when streaming
  $effect(() => {
    if ($chatStore.isStreaming && contentEl) {
      contentEl.scrollTop = contentEl.scrollHeight;
    }
  });

  // Adjust position to prevent overflow
  $effect(() => {
    if (panelEl) {
      const rect = panelEl.getBoundingClientRect();
      const windowWidth = window.innerWidth;
      
      // If panel overflows on the right, move it to the left
      if (side === 'right' && rect.right > windowWidth - 10) {
        panelEl.style.marginRight = 'auto';
        panelEl.style.marginLeft = '10px';
      }
      // If panel overflows on the left, move it to the right
      else if (side === 'left' && rect.left < 10) {
        panelEl.style.marginLeft = 'auto';
        panelEl.style.marginRight = '10px';
      } else {
        panelEl.style.marginRight = '';
        panelEl.style.marginLeft = '';
      }
    }
  });

  let isError = $derived(!$chatStore.isStreaming && !!$chatStore.error);
  let isStreaming = $derived($chatStore.isStreaming);
  let isBusy = $derived($chatStore.isStreaming || $characterState === 'thinking');

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && inputText.trim() && !isBusy) {
      e.preventDefault();
      onSend(inputText.trim());
      inputText = '';
    }
  }

  function handleSendClick() {
    if (inputText.trim() && !isBusy) {
      onSend(inputText.trim());
      inputText = '';
    }
  }
</script>

<div
  bind:this={panelEl}
  class="chat-panel tail-{side}"
  in:fly={{ x: side === 'right' ? -16 : 16, duration: 200, opacity: 0 }}
  out:fly={{ x: side === 'right' ? -12 : 12, duration: 160, opacity: 0 }}
>
  <!-- Close button (floating, no title bar) -->
  <button class="close-btn" onclick={onCollapse} title="收起" aria-label="收起聊天窗口">✕</button>

  <!-- Content area -->
  <div class="content-area" bind:this={contentEl} class:error={isError}>
    <div class="message-content">
      {#if $chatStore.isStreaming && $chatStore.thinkingSteps.length > 0 && !$chatStore.streamingContent}
        <!-- LLM thinking/tool-calling phase: show intermediate steps -->
        <div class="thinking-steps">
          {#each $chatStore.thinkingSteps as step}
            <span class="thinking-step">{step}</span>
          {/each}
        </div>
        <span class="cursor" aria-hidden="true">▋</span>
      {:else if $chatStore.isStreaming}
        <p class="message-text" class:error-text={isError}>
          {@html sanitizeHtml($chatStore.streamingContent)}<span class="cursor" aria-hidden="true">▋</span>
        </p>
      {:else}
        <p class="message-text" class:error-text={isError}>
          {@html sanitizeHtml($chatStore.error ?? $chatStore.messages.at(-1)?.content ?? t($settingsStore.tts_primary_voice).hint.replace("{key}", $settingsStore.hotkey_name))}
        </p>
      {/if}
    </div>
  </div>

  <!-- Input area -->
  <div class="input-area">
    <input
      type="text"
      placeholder={isBusy ? t($settingsStore.tts_primary_voice).inputBusy : t($settingsStore.tts_primary_voice).inputPlaceholder}
      bind:value={inputText}
      onkeydown={handleKeydown}
      disabled={isBusy}
      maxlength={500}
      autocomplete="off"
      spellcheck="false"
      aria-label="消息输入"
    />
    <button
      class="send-btn"
      onclick={handleSendClick}
      disabled={isBusy || !inputText.trim()}
      aria-label="发送"
    >
      ↑
    </button>
  </div>
</div>

<style>
  /* ─── Panel ─── */
  .chat-panel {
    width: 280px;
    height: 135px;
    display: flex;
    flex-direction: column;
    background: rgba(10, 10, 22, 0.82);
    -webkit-backdrop-filter: blur(24px) saturate(160%);
    backdrop-filter: blur(24px) saturate(160%);
    border: 1px solid rgba(160, 168, 255, 0.28);
    border-radius: 10px;
    overflow: hidden;
    position: relative;
    flex-shrink: 0;
    align-self: center;
  }

  /* ─── Floating close button ─── */
  .close-btn {
    position: absolute;
    top: 5px;
    right: 6px;
    width: 18px;
    height: 18px;
    border-radius: 4px;
    background: none;
    border: none;
    color: rgba(232, 232, 240, 0.28);
    font-size: 10px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background 0.12s, color 0.12s;
    z-index: 2;
    line-height: 1;
    padding: 0;
  }
  .close-btn:hover {
    background: rgba(255, 255, 255, 0.08);
    color: rgba(232, 232, 240, 0.7);
  }

  /* ─── Bubble tail ─── */
  /* tail-right = panel on the RIGHT of avatar, tail points LEFT */
  .chat-panel.tail-right::before,
  .chat-panel.tail-right::after {
    content: '';
    position: absolute;
    top: 20px;
    left: -9px;
    width: 0;
    height: 0;
    border-style: solid;
  }
  .chat-panel.tail-right::after {
    border-width: 8px 9px 8px 0;
    border-color: transparent rgba(10, 10, 22, 0.82) transparent transparent;
  }
  .chat-panel.tail-right::before {
    left: -11px;
    border-width: 9px 10px 9px 0;
    border-color: transparent rgba(160, 168, 255, 0.28) transparent transparent;
  }

  /* tail-left = panel on the LEFT of avatar, tail points RIGHT */
  .chat-panel.tail-left::before,
  .chat-panel.tail-left::after {
    content: '';
    position: absolute;
    top: 20px;
    right: -9px;
    width: 0;
    height: 0;
    border-style: solid;
  }
  .chat-panel.tail-left::after {
    border-width: 8px 0 8px 9px;
    border-color: transparent transparent transparent rgba(10, 10, 22, 0.82);
  }
  .chat-panel.tail-left::before {
    right: -11px;
    border-width: 9px 0 9px 10px;
    border-color: transparent transparent transparent rgba(160, 168, 255, 0.28);
  }

  /* ─── Content area ─── */
  .content-area {
    flex: 1;
    padding: 8px 28px 4px 12px; /* right pad avoids close btn overlap */
    overflow-y: auto;
    scrollbar-width: thin;
    scrollbar-color: rgba(160, 168, 255, 0.25) transparent;
    position: relative;
    min-height: 0;
  }
  .content-area::-webkit-scrollbar       { width: 3px; }
  .content-area::-webkit-scrollbar-track { background: transparent; }
  .content-area::-webkit-scrollbar-thumb { background: rgba(160, 168, 255, 0.25); border-radius: 2px; }

  .message-text {
    margin: 0;
    font-size: 12px;
    line-height: 1.6;
    color: rgba(232, 232, 240, 0.92);
    word-break: break-word;
    white-space: pre-wrap;
  }
  .message-text :global(a) {
    color: rgba(124, 158, 255, 0.9);
    text-decoration: underline;
    cursor: pointer;
    transition: color 0.15s;
  }
  .message-text :global(a):hover {
    color: rgba(160, 168, 255, 1);
  }
  .error-text { color: rgba(255, 120, 120, 0.9) !important; }

  .message-content { width: 100%; }

  .thinking-steps {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .thinking-step {
    font-size: 11px;
    line-height: 1.5;
    color: rgba(160, 168, 255, 0.7);
    animation: fade-in-step 0.3s ease-out;
    word-break: break-word;
  }

  @keyframes fade-in-step {
    from { opacity: 0; transform: translateY(-4px); }
    to   { opacity: 1; transform: translateY(0); }
  }

  @keyframes blink-cursor {
    0%, 100% { opacity: 1; }
    50%       { opacity: 0; }
  }
  .cursor {
    display: inline-block;
    color: var(--primary);
    animation: blink-cursor 0.75s ease-in-out infinite;
  }

  /* ─── Input area ─── */
  .input-area {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 4px 6px 6px;
    border-top: 1px solid rgba(255, 255, 255, 0.06);
    flex-shrink: 0;
  }

  input {
    flex: 1;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 7px;
    padding: 4px 9px;
    color: rgba(232, 232, 240, 0.92);
    font-size: 11.5px;
    outline: none;
    transition: border-color 0.15s;
    min-width: 0;
  }
  input:focus       { border-color: rgba(160, 168, 255, 0.5); }
  input:disabled    { opacity: 0.4; cursor: not-allowed; }
  input::placeholder { color: rgba(232, 232, 240, 0.28); }

  .send-btn {
    width: 26px;
    height: 26px;
    border-radius: 6px;
    background: rgba(160, 168, 255, 0.18);
    border: 1px solid rgba(160, 168, 255, 0.32);
    color: #A0A8FF;
    font-size: 14px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background 0.12s, opacity 0.12s;
    flex-shrink: 0;
    line-height: 1;
    padding: 0;
  }
  .send-btn:hover:not(:disabled) { background: rgba(160, 168, 255, 0.32); }
  .send-btn:disabled { opacity: 0.3; cursor: not-allowed; }
</style>
