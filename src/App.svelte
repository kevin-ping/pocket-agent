<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { invoke } from '@tauri-apps/api/core';
  import { getCurrentWindow, LogicalPosition } from '@tauri-apps/api/window';

  import AvatarIcon from './lib/components/AvatarIcon.svelte';
  import DynamicIsland from './lib/components/DynamicIsland.svelte';
  import Icon from './lib/components/Icon.svelte';
  import ChatPanel from './lib/components/ChatPanel.svelte';
  import SettingsPanel from './lib/components/SettingsPanel.svelte';

  import { characterState } from './lib/stores/character';
  import { chatStore } from './lib/stores/chat';
  import { settingsStore } from './lib/stores/settings';
  import { layoutStore } from './lib/stores/layout';

  const appWindow = getCurrentWindow();

  // ─── Drag (avatar only) ───
  function handleAvatarDragStart(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (target.closest('button, input, select, [role="textbox"]')) return;
    appWindow.startDragging();
  }

  // ─── Chat send ───
  async function handleSendMessage(text: string, userLanguage?: string) {
    chatStore.addUserMessage(text);
    chatStore.startStream();
    characterState.toThinking();
    islandMode = 'thinking';
    spiritPhase = 2;
    firstStreamDelta = false;
    try {
      await invoke('send_message', {
        text,
        ttsFormat: get(settingsStore).tts_format,
        ttsPrimaryVoice: get(settingsStore).tts_primary_voice,
        ttsAux1Voice: get(settingsStore).tts_aux1_voice,
        ttsAux2Voice: get(settingsStore).tts_aux2_voice,
        userLanguage: userLanguage || 'zh',
        fixedLang: get(settingsStore).fixed_lang || '',
        ttsEnabled: get(settingsStore).tts_enabled,
      });
    } catch (e) {
      chatStore.setError(`连接失败: ${e}`);
      characterState.toIdle();
      spiritPhase = 0;
    }
  }

  // ─── Context menu ───
  let muted = false;
  let islandMode: "idle" | "recording" | "thinking" = "idle";
  let spiritPhase = 0;
  let firstStreamDelta = false;

  // ─── Settings panel ───
  let showSettings = false;
  let prevWindowState: { x: number; y: number; w: number; h: number } | null = null;

  async function openSettings() {
    prevWindowState = await layoutStore.openSettings();
    // Wait for WebView to process the window resize before rendering the panel
    await new Promise<void>(resolve => requestAnimationFrame(resolve));
    showSettings = true;
  }

  async function closeSettings() {
    showSettings = false;
    if (prevWindowState) {
      await layoutStore.closeSettings(prevWindowState);
      prevWindowState = null;
    }
  }

  // ─── Accessibility guide ───
  let showAccessibilityGuide = false;

  // ─── Event listeners ───
  let unlisten: UnlistenFn[] = [];

  async function setupListeners() {
    unlisten = await Promise.all([
      listen('chat-thinking-start', () => { characterState.toThinking(); chatStore.addThinkingStep('🤔 正在思考...'); }),
      listen<{ emotion: string; total_chars: number; has_audio: boolean }>('chat-speaking-start', (e) => {
        characterState.toSpeaking();
        chatStore.clearThinkingSteps();
        spiritPhase = 0;
        chatStore.startStream();
        chatStore.startTypewriter(e.payload.emotion);
        if (!e.payload.has_audio && !$layoutStore.expanded) {
          layoutStore.toggle();
        }
      }),
      listen<{ delta: string }>('chat-stream', (e) => {
        chatStore.appendDelta(e.payload.delta);
        if (!firstStreamDelta) {
          firstStreamDelta = true;
          spiritPhase = Math.max(spiritPhase, 2);
        }
      }),
      listen('chat-stream-end', () => {
        islandMode = 'idle';
        chatStore.endStream();
        if (get(characterState) !== 'speaking') {
          characterState.toIdle();
          spiritPhase = 0;
        }
      }),
      listen('chat-audio-done', () => {
        characterState.transition('speaking', 'idle');
        spiritPhase = 0;
      }),

      // LLM intermediate thinking/reasoning updates (in-place update of last 🤔 step)
      listen<string>('chat-thinking', (e) => {
        chatStore.updateLastThinkingStep(e.payload);
      }),

      // Tool call start notification
      listen<string>('chat-tool-call', (e) => {
        try {
          const payload = JSON.parse(e.payload);
          // Clean up tool name: strip common prefixes
          let toolName = payload.name
            .replace(/^mcp_tradingview_/, '')
            .replace(/^mcp_/, '')
            .replace(/_/g, ' ');
          chatStore.addThinkingStep(`🔧 查询 ${toolName}...`);
        } catch {
          chatStore.addThinkingStep(`🔧 正在执行操作...`);
        }
      }),
      listen<string>('chat-stream-error', (e) => {
        islandMode = 'idle';
        chatStore.setError(e.payload);
        characterState.toIdle();
        spiritPhase = 0;
      }),

      listen('fn-key-down', () => {
        islandMode = 'recording';
        spiritPhase = 0;
        firstStreamDelta = false;
        characterState.toListening();
        chatStore.clear();
        invoke('start_voice_recording').catch(console.error);
      }),
      listen('fn-key-up', () => {
        islandMode = 'thinking';
        spiritPhase = 1;
        characterState.toThinking();
        invoke('stop_voice_recording').catch(console.error);
      }),
      listen('voice-cancel', () => {
        islandMode = 'idle';
        spiritPhase = 0;
        characterState.toIdle();
        invoke('cancel_voice_recording').catch(console.error);
      }),
      listen<{ text: string; language: string }>('stt-result', (e) => {
        islandMode = 'thinking';
        if (e.payload.text.trim()) {
          handleSendMessage(e.payload.text, e.payload.language);
        } else {
          spiritPhase = 0;
          characterState.toIdle();
        }
      }),
      listen<{ error: string }>('stt-error', (e) => {
        islandMode = 'idle';
        spiritPhase = 0;
        console.warn('[STT]', e.payload.error);
        characterState.toIdle();
      }),

      listen('accessibility-permission-required', () => {
        showAccessibilityGuide = true;
      }),
      listen('tray-open-settings', () => {
        openSettings();
      }),

      // API push: external message pushed to PA (e.g. from Hermes cron)
      // Only call speak_text — text display is handled by the speak_text
      // Rust side which emits chat-speaking-start (typewriter) + chat-stream (delta).
      listen<{ text: string; emotion: string; voice: string | null }>("api-push", (e) => {
        const { text, emotion, voice } = e.payload;
        if (!text.trim()) return;
        spiritPhase = 3;
        firstStreamDelta = false;
        invoke("speak_text", {
          text,
          emotion,
          overrideVoice: voice || undefined,
          ttsFormat: get(settingsStore).tts_format,
          ttsPrimaryVoice: get(settingsStore).tts_primary_voice,
          ttsAux1Voice: get(settingsStore).tts_aux1_voice,
          ttsAux2Voice: get(settingsStore).tts_aux2_voice,
          ttsEnabled: get(settingsStore).tts_enabled,
        }).catch(console.error);
      }),
    ]);
  }

  // ─── Save window position on drag + on close ───
  let posDragTimer: ReturnType<typeof setTimeout> | null = null;

  async function saveCurrentPosition() {
    let pos, scale;
    try {
      pos = await appWindow.outerPosition();
      scale = await appWindow.scaleFactor();
    } catch { return; }
    if (scale <= 0) return;
    const layout = get(layoutStore);
    let avatarX = pos.x / scale;
    if (layout.expanded && layout.avatarSide === 'right') {
      avatarX = pos.x / scale + (layoutStore.EXPANDED_W - layoutStore.AVATAR_W);
    }
    try {
      await settingsStore.save({ window_x: Math.round(avatarX), window_y: Math.round(pos.y / scale) });
    } catch {}
  }

  // ─── Listen for window drag + save on close ───
  async function setupWindowPositionSave() {
    // Debounced save on window move (fires during drag)
    const unlistenMove = await appWindow.onMoved(() => {
      if (posDragTimer) clearTimeout(posDragTimer);
      posDragTimer = setTimeout(() => saveCurrentPosition(), 500);
    });
    unlisten.push(unlistenMove);

    // Final save on close
    await appWindow.onCloseRequested(async () => {
      if (posDragTimer) clearTimeout(posDragTimer);
      await saveCurrentPosition();
    });
  }

  // ─── Restore window position on mount ───
  async function restoreWindowPosition() {
    const settings = get(settingsStore);
    if (settings.window_x !== null && settings.window_y !== null) {
      try {
        await appWindow.setPosition(new LogicalPosition(settings.window_x, settings.window_y));
      } catch (e) { console.warn('[POS] restore failed:', e); }
    }
  }

  onMount(async () => {
    await settingsStore.load();
    await restoreWindowPosition();
    await setupListeners();
    await setupWindowPositionSave();
  });

  onDestroy(() => {
    unlisten.forEach((fn) => fn());
  });
</script>

<!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
<main
  class="app-root"
  class:expanded={$layoutStore.expanded}
  class:avatar-right={$layoutStore.avatarSide === 'right'}
  role="application"
  aria-label="Pocket Agent"
>
  <!-- Chat panel on LEFT (when avatar is on the right side of screen) -->
  {#if $layoutStore.expanded && $layoutStore.avatarSide === 'right'}
    <ChatPanel
      side="left"
      onSend={handleSendMessage}
      onCollapse={() => layoutStore.toggle()}
    />
    <div class="gap"></div>
  {/if}

  <!-- Avatar icon (always visible, handles drag) -->
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <div
    class="avatar-zone"
    on:mousedown={handleAvatarDragStart}
  >
    <AvatarIcon
      avatarImage={$settingsStore.avatar_image ?? null}
      spiritPhase={spiritPhase}
      on:expand={() => layoutStore.toggle()}
    />
    <DynamicIsland mode={islandMode} />
  </div>

  <!-- Chat panel on RIGHT (default, when avatar is on the left side of screen) -->
  {#if $layoutStore.expanded && $layoutStore.avatarSide === 'left'}
    <div class="gap"></div>
    <ChatPanel
      side="right"
      onSend={handleSendMessage}
      onCollapse={() => layoutStore.toggle()}
    />
  {/if}


  <!-- Settings panel (takes over window when open) -->
  {#if showSettings}
    <SettingsPanel bind:visible={showSettings} onclose={closeSettings} />
  {/if}

  <!-- Accessibility guide overlay -->
  {#if showAccessibilityGuide}
    <div class="permission-guide" role="alert">
      <p class="guide-title"><Icon name="alert-triangle" size={14} color="rgba(255, 200, 80, 0.95)" /> 需要辅助功能权限</p>
      <p class="guide-body">
        🍎 左上角苹果菜单 → 系统设置 → 隐私与安全性 → 辅助功能<br />
        在右侧列表中找到 Pocket Agent 并打开开关，然后重启应用
      </p>
      <button class="guide-btn" on:click={() => (showAccessibilityGuide = false)}>
        我知道了
      </button>
    </div>
  {/if}
</main>

<style>
  :global(*, *::before, *::after) { box-sizing: border-box; }
  :global(body) {
    margin: 0;
    padding: 0;
    background: transparent;
    overflow: hidden;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
    -webkit-font-smoothing: antialiased;
  }
  :global(:root) {
    --primary: #A0A8FF;
    --primary-rgb: 160, 168, 255;
    --bg-panel: rgba(14, 14, 26, 0.94);
    --text: rgba(232, 232, 240, 0.92);
  }

  .app-root {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: row;
    align-items: center;
    background: transparent;
    position: relative;
  }

  .avatar-zone {
    width: 108px;
    height: 146px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: flex-start;
    flex-shrink: 0;
    position: relative;
    overflow: visible;
  }

  /* Vertical centering: avatar is 112px (incl. label), panel is 120px */
  .app-root.expanded .avatar-zone {
    align-self: flex-start;
    /* margin-top: calc((120px - 126px) / 2); */
  }

  .gap {
    width: 12px;
    flex-shrink: 0;
  }

  /* ─── Accessibility guide ─── */
  .permission-guide {
    position: fixed;
    inset: 12px;
    z-index: 50;
    background: rgba(14, 14, 26, 0.97);
    border: 1px solid rgba(255, 180, 50, 0.4);
    border-radius: 14px;
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.7);
  }
  .guide-title {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: rgba(255, 200, 80, 0.95);
  }
  .guide-body {
    margin: 0;
    font-size: 12px;
    line-height: 1.7;
    color: rgba(232, 232, 240, 0.75);
  }
  .guide-btn {
    align-self: flex-end;
    padding: 5px 16px;
    background: rgba(160, 168, 255, 0.2);
    border: 1px solid rgba(160, 168, 255, 0.4);
    border-radius: 8px;
    color: #d0dcff;
    font-size: 12px;
    cursor: pointer;
    transition: background 0.1s;
  }
  .guide-btn:hover { background: rgba(160, 168, 255, 0.35); }
</style>
