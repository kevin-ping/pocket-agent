<script lang="ts">
  import { chatStore } from '../stores/chat';
  import { characterState } from '../stores/character';
  import { settingsStore } from '../stores/settings';
  import { t } from '../i18n';

  export let dialogStyle: 'bubble' | 'tv' | 'terminal' = 'bubble';
  export let onSend: (text: string) => void;

  let inputText = '';
  let inputEl: HTMLInputElement;

  // 流式时显示累积内容，结束后显示最新一条消息，无消息时显示欢迎语
  $: displayContent = $chatStore.isStreaming
    ? $chatStore.streamingContent
    : ($chatStore.error ?? $chatStore.messages.at(-1)?.content ?? t($settingsStore.tts_primary_voice).hint.replace("{key}", $settingsStore.hotkey_name));

  $: isError = !$chatStore.isStreaming && !!$chatStore.error;
  $: isStreaming = $chatStore.isStreaming;
  $: isBusy = $chatStore.isStreaming || $characterState === 'thinking';

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && inputText.trim() && !isBusy) {
      e.preventDefault();
      onSend(inputText.trim());
      inputText = '';
    }
  }
</script>

<div class="dialog-box style-{dialogStyle}" class:error={isError}>
  <div class="content-area">
    <p class="message-text" class:error-text={isError}>
      {displayContent}{#if isStreaming}<span class="cursor" aria-hidden="true">▋</span>{/if}
    </p>
  </div>

  <div class="input-area">
    <input
      bind:this={inputEl}
      type="text"
      placeholder={isBusy ? t($settingsStore.tts_primary_voice).inputBusy : t($settingsStore.tts_primary_voice).inputPlaceholder}
      bind:value={inputText}
      on:keydown={handleKeydown}
      disabled={isBusy}
      maxlength={500}
      autocomplete="off"
      spellcheck="false"
    />
  </div>
</div>

<style>
  /* ─── 共用结构 ─── */
  .dialog-box {
    width: 100%;
    display: flex;
    flex-direction: column;
    border-radius: 14px;
    overflow: hidden;
    /* 不用 backdrop-filter：macOS 10.13 WKWebView 支持不稳定 */
  }

  /* ─── bubble 皮肤（默认）─── */
  .style-bubble {
    background: rgba(18, 18, 32, 0.90);
    border: 1px solid rgba(124, 158, 255, 0.28);
  }

  /* ─── terminal 皮肤 ─── */
  .style-terminal {
    background: rgba(0, 8, 0, 0.94);
    border: 1px solid #00c136;
    border-radius: 4px;
    font-family: 'Courier New', Courier, monospace;
  }
  .style-terminal .message-text { color: #00e642; font-size: 12px; }
  .style-terminal .cursor       { color: #00e642; }
  .style-terminal input {
    border-color: #00c136;
    color: #00e642;
    background: rgba(0, 30, 0, 0.7);
  }
  .style-terminal input::placeholder { color: rgba(0, 200, 60, 0.4); }

  /* ─── tv 皮肤（Phase 3 CSS 完善，这里先给基础框）─── */
  .style-tv {
    background: rgba(20, 14, 8, 0.94);
    border: 3px solid #8b6914;
    border-radius: 8px;
    outline: 2px solid #3a2800;
  }

  /* ─── 错误状态 ─── */
  .dialog-box.error {
    border-color: rgba(255, 80, 80, 0.4);
  }
  .error-text { color: rgba(255, 120, 120, 0.9) !important; }

  /* ─── 内容区 ─── */
  .content-area {
    flex: 1;
    min-height: 120px;
    max-height: 180px;
    padding: 12px 14px 8px;
    overflow-y: auto;
    /* 自定义滚动条，macOS 10.13 支持 */
    scrollbar-width: thin;
    scrollbar-color: rgba(124, 158, 255, 0.3) transparent;
  }
  .content-area::-webkit-scrollbar       { width: 4px; }
  .content-area::-webkit-scrollbar-track { background: transparent; }
  .content-area::-webkit-scrollbar-thumb { background: rgba(124, 158, 255, 0.3); border-radius: 2px; }

  .message-text {
    margin: 0;
    font-size: 13px;
    line-height: 1.65;
    color: rgba(232, 232, 240, 0.92);
    word-break: break-word;
    white-space: pre-wrap;
  }

  /* 打字机光标：只用 opacity 动画，不触发 layout */
  @keyframes blink-cursor {
    0%, 100% { opacity: 1; }
    50%       { opacity: 0; }
  }
  .cursor {
    display: inline-block;
    color: #7c9eff;
    animation: blink-cursor 0.75s ease-in-out infinite;
    /* will-change 不设置：光标极小，不值得独立合成层 */
  }

  /* ─── 输入区 ─── */
  .input-area {
    padding: 6px 8px 8px;
    border-top: 1px solid rgba(255, 255, 255, 0.07);
  }

  input {
    width: 100%;
    box-sizing: border-box;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid rgba(255, 255, 255, 0.10);
    border-radius: 8px;
    padding: 6px 10px;
    color: rgba(232, 232, 240, 0.92);
    font-size: 12px;
    outline: none;
    transition: border-color 0.15s;
    /* transition 只作用于 border-color，不触发 layout，旧机器安全 */
  }
  input:focus  { border-color: rgba(124, 158, 255, 0.5); }
  input:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  input::placeholder { color: rgba(232, 232, 240, 0.3); }
</style>
