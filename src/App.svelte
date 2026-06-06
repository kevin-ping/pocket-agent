<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { invoke } from '@tauri-apps/api/core';
  import { getCurrentWindow, LogicalPosition, LogicalSize } from '@tauri-apps/api/window';

  import AvatarIcon from './lib/components/AvatarIcon.svelte';
  import DynamicIsland from './lib/components/DynamicIsland.svelte';
  import Icon from './lib/components/Icon.svelte';
  import ChatPanel from './lib/components/ChatPanel.svelte';
  import SettingsPanel from './lib/components/SettingsPanel.svelte';
  import StatusPanel from './lib/components/StatusPanel.svelte';
  import BreakConfirmModal from './lib/components/BreakConfirmModal.svelte';

  import { characterState } from './lib/stores/character';
  import { chatStore } from './lib/stores/chat';
  import { settingsStore } from './lib/stores/settings';
  import { layoutStore } from './lib/stores/layout';
  import { STATUS_PHRASES, langFromVoice, detectLang, convLabels, type LangKey } from './lib/i18n';

  const appWindow = getCurrentWindow();

  // ─── Drag (avatar only) ───
  function handleAvatarDragStart(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (target.closest('button, input, select, [role="textbox"]')) return;
    appWindow.startDragging();
  }

  // ─── Chat send ───
  async function handleSendMessage(text: string, userLanguage?: string) {
    if (isTurnActive()) {
      requestNewTurn(() => { handleSendMessageImpl(text, userLanguage); });
      return;
    }
    return handleSendMessageImpl(text, userLanguage);
  }

  async function handleSendMessageImpl(text: string, userLanguage?: string) {
    // Update current input language for status TTS (explicit param from STT wins,
    // otherwise detect from typed text).
    currentInputLang = (userLanguage as LangKey | undefined) ?? detectLang(text);

    chatStore.addUserMessage(text);
    // Save user message to history
    invoke('save_chat_message', { role: 'user', content: text }).catch(e => console.error('Failed to save user message:', e));

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
  let islandMode: "idle" | "recording" | "thinking" | "waiting_for_wake" | "verifying_speaker" = "idle";
  let spiritPhase = 0;
  let audioLevel = 0;
  let audioLevelTimer: ReturnType<typeof setInterval> | null = null;
  let firstStreamDelta = false;
  let lastSpeakingHadAudio = true;
  let bridgeThinkingActive = false;

  // ─── Continuous conversation mode ───
  let conversationActive = false;
  let bridgeFallbackTimer: ReturnType<typeof setTimeout> | null = null;

  function voiceListeningText(): string {
    return `🎤 ${convLabels(get(settingsStore).tts_primary_voice).voiceListening}`;
  }

  function clearBridgeFallbackTimer() {
    if (bridgeFallbackTimer) {
      clearTimeout(bridgeFallbackTimer);
      bridgeFallbackTimer = null;
    }
  }

  function stopBridgeThinking(message?: string) {
    clearBridgeFallbackTimer();
    if (!bridgeThinkingActive) return;
    bridgeThinkingActive = false;
    islandMode = 'idle';
    spiritPhase = 0;
    cancelPendingStatusSpeech();
    lastSpokenStatus = null;
    chatStore.clearThinkingSteps();
    characterState.toIdle();
    if (message) {
      chatStore.setError(message);
    }
  }

  function startBridgeThinking() {
    bridgeThinkingActive = true;
    clearBridgeFallbackTimer();
    islandMode = 'thinking';
    spiritPhase = 2;
    firstStreamDelta = false;
    characterState.toThinking();
    chatStore.addThinkingStep('🤔 正在思考...');
    cancelPendingStatusSpeech();
    pendingStatusSpeech = setTimeout(() => speakStatus({ kind: 'thinking' }), 250);
    bridgeFallbackTimer = setTimeout(() => {
      stopBridgeThinking('暂时还没有收到 Hermes 回推');
    }, 30000);
  }

  const DEBUG_UI_STATE = String(import.meta.env.VITE_PA_DEBUG_UI || '').toLowerCase() === 'true';
  function debugState(event: string, extra?: Record<string, unknown>) {
    if (!DEBUG_UI_STATE) return;
    const now = new Date().toISOString().slice(11, 23);
    console.debug(`[PA-UI][${now}] ${event}`, {
      islandMode,
      spiritPhase,
      firstStreamDelta,
      bridgeThinkingActive,
      characterState: get(characterState),
      chatStreaming: $chatStore.isStreaming,
      messageCount: $chatStore.messages.length,
      ...(extra || {}),
    });
  }

  // When typewriter finishes (isStreaming→false) with no audio, stop SPEAKING animation
  $: if (!$chatStore.isStreaming && $chatStore.messages.length > 0 && !lastSpeakingHadAudio) {
    const cs = get(characterState);
    if (cs === 'speaking') {
      characterState.transition('speaking', 'idle');
      spiritPhase = 0;
      lastSpeakingHadAudio = true;  // reset guard
    }
  }

  // Grow window vertically to fit the status panel while expanded so 🤔 + 🔧 lines stay visible.
  const STATUS_BASE_H = 188;     // matches layout.ts CHAT_H
  const STATUS_LINE_H = 17;      // 10.5 px font * 1.45 line-height ≈ 15 + 2 px gap
  const STATUS_BLOCK_PAD = 18;   // 6+6 content padding + 6 .status-row margin-top
  const STATUS_EXPANDED_W = 400; // 108 (AVATAR_W) + 12 (GAP) + 280 (CHAT_W)
  $: extraStatusH = $chatStore.thinkingSteps.length === 0
    ? 0
    : STATUS_BLOCK_PAD + $chatStore.thinkingSteps.length * STATUS_LINE_H;
  $: if ($layoutStore.expanded && !$layoutStore.resizing) {
    const targetH = STATUS_BASE_H + extraStatusH;
    queueMicrotask(() => {
      appWindow.setSize(new LogicalSize(STATUS_EXPANDED_W, targetH)).catch(() => {});
    });
  }

  // ─── Settings panel ───
  let showSettings = false;
  let prevWindowState: { x: number; y: number; w: number; h: number } | null = null;

  async function openSettings() {
    prevWindowState = await layoutStore.openSettings();
    // Wait for WebView to process the window resize before rendering the panel
    await new Promise<void>(resolve => requestAnimationFrame(resolve));
    showSettings = true;
  }

  async function openChatHistory() {
    try {
      await invoke('open_chat_history');
    } catch (e) {
      console.error('Failed to open chat history:', e);
    }
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

  // ─── Status TTS ───
  let pendingStatusSpeech: ReturnType<typeof setTimeout> | null = null;
  let lastSpokenStatus: string | null = null;
  // Most-recent user-input language; status TTS uses this so spoken phrases
  // match the language the user is actually using.
  let currentInputLang: LangKey = 'zh';

  function cancelPendingStatusSpeech() {
    if (pendingStatusSpeech) { clearTimeout(pendingStatusSpeech); pendingStatusSpeech = null; }
  }

  // ─── Wake-word state (FEAT-C4) ───
  let wakeArmInFlight = false;

  // stt-server starts on a background thread and is not ready at app mount
  // (Whisper model load + token file write take ~1-3 s). The wake listener's
  // first connect attempt loses this race and fails with either
  // "ws auth token: ... No such file" or "ws connect: Connection refused".
  // Retry on those transient errors so wake comes up automatically once the
  // server is healthy, without forcing the user to toggle the switch.
  async function armWakeListenerIfEnabled() {
    const s = get(settingsStore);
    if (!s.wake_word_enabled) return;
    if (wakeArmInFlight) return;
    wakeArmInFlight = true;
    try {
      const MAX_ATTEMPTS = 20;
      for (let i = 0; i < MAX_ATTEMPTS; i++) {
        try {
          await invoke('start_wake_word_listening', { threshold: s.wake_word_threshold });
          if (!conversationActive && islandMode === 'idle') {
            islandMode = 'waiting_for_wake';
          }
          return;
        } catch (e) {
          const msg = String(e ?? '');
          // Idempotent ignore: "already active" races on conversation-ended.
          if (msg.includes('already active')) return;
          // Transient: stt-server not ready yet, or capture still held by
          // previous owner (async release). Backoff 500 ms and retry.
          if (
            msg.includes('ws connect') ||
            msg.includes('ws auth token') ||
            msg.includes('read server token failed') ||
            msg.includes('capture busy') ||
            msg.includes('mic capture:')
          ) {
            if (i === MAX_ATTEMPTS - 1) {
              console.warn('[wake] arm gave up after retries:', msg);
              return;
            }
            await new Promise((r) => setTimeout(r, 500));
            continue;
          }
          // Non-transient failure: surface once and stop.
          console.warn('[wake] arm failed', e);
          return;
        }
      }
    } finally {
      wakeArmInFlight = false;
    }
  }

  async function disarmWakeListener() {
    try {
      await invoke('stop_wake_word_listening');
    } catch (e) {
      console.warn('[wake] disarm failed', e);
    }
    if (islandMode === 'waiting_for_wake') islandMode = 'idle';
  }


  // ─── Break / interrupt current turn ───
  let showBreakConfirm = false;
  let pendingBreakAction: (() => void) | null = null;

  function isTurnActive(): boolean {
    const s = get(chatStore);
    return s.isStreaming || s.thinkingSteps.length > 0 || bridgeThinkingActive;
  }

  function requestNewTurn(action: () => void) {
    if (!isTurnActive()) { action(); return; }
    const s = get(settingsStore);
    if (s.continuous_conversation && s.skip_interrupt_confirmation) {
      // Skip confirm popup; immediately break and run action.
      confirmBreakSilently(action);
      return;
    }
    pendingBreakAction = action;
    showBreakConfirm = true;
  }

  async function confirmBreakSilently(action: () => void) {
    cancelPendingStatusSpeech();
    lastSpokenStatus = null;
    try { await invoke('discard_pending_turn'); } catch (e) { console.error(e); }
    chatStore.endStream();
    chatStore.clearThinkingSteps();
    bridgeThinkingActive = false;
    clearBridgeFallbackTimer();
    islandMode = 'idle';
    spiritPhase = 0;
    characterState.toIdle();
    action();
  }

  async function confirmBreak() {
    showBreakConfirm = false;
    cancelPendingStatusSpeech();
    lastSpokenStatus = null;
    try { await invoke('discard_pending_turn'); } catch (e) { console.error(e); }
    chatStore.endStream();
    chatStore.clearThinkingSteps();
    bridgeThinkingActive = false;
    clearBridgeFallbackTimer();
    islandMode = 'idle';
    spiritPhase = 0;
    characterState.toIdle();
    const action = pendingBreakAction;
    pendingBreakAction = null;
    action?.();
  }

  async function cancelBreak() {
    showBreakConfirm = false;
    pendingBreakAction = null;
    // The hotkey thread already flipped is_active=true before the popup showed,
    // so reset it here — otherwise the next press is interpreted as "stop".
    try { await invoke('reset_hotkey_active_state'); } catch (e) { console.error(e); }
  }

  function autoCloseBreakConfirm() {
    if (showBreakConfirm) cancelBreak();
  }

  // Resolve which language status TTS should use this turn.
  // Forced language wins; otherwise fall back to whatever the user just typed/said.
  function getStatusLang(): LangKey {
    const s = get(settingsStore);
    if (s.fixed_lang === 'primary' && s.tts_primary_voice) return langFromVoice(s.tts_primary_voice);
    if (s.fixed_lang === 'aux1' && s.tts_aux1_voice) return langFromVoice(s.tts_aux1_voice);
    if (s.fixed_lang === 'aux2' && s.tts_aux2_voice) return langFromVoice(s.tts_aux2_voice);
    return currentInputLang;
  }

  // Pick a configured voice that matches the target language; undefined → backend auto-picks.
  function pickVoiceForLang(lang: LangKey): string | undefined {
    const s = get(settingsStore);
    for (const v of [s.tts_primary_voice, s.tts_aux1_voice, s.tts_aux2_voice]) {
      if (v && langFromVoice(v) === lang) return v;
    }
    return undefined;
  }

  type StatusKind = { kind: 'thinking' } | { kind: 'querying'; name: string } | { kind: 'executing' } | { kind: 'running-command' };
  function speakStatus(kind: StatusKind) {
    cancelPendingStatusSpeech();
    const s = get(settingsStore);
    if (!s.tts_enabled) return;
    const lang = getStatusLang();
    const p = STATUS_PHRASES[lang] ?? STATUS_PHRASES.zh;
    const text = kind.kind === 'thinking' ? p.thinking
               : kind.kind === 'executing' ? p.executing
               : kind.kind === 'running-command' ? p.runningCommand
               : p.querying(kind.name);
    if (!text || text === lastSpokenStatus) return;
    lastSpokenStatus = text;
    invoke("speak_status", {
      text,
      overrideVoice: pickVoiceForLang(lang),
      ttsFormat: s.tts_format,
      ttsPrimaryVoice: s.tts_primary_voice,
      ttsAux1Voice: s.tts_aux1_voice,
      ttsAux2Voice: s.tts_aux2_voice,
      ttsEnabled: s.tts_enabled,
    }).catch(console.error);
  }

  async function setupListeners() {
    chatStore.setOnCmdDetected(() => {
      chatStore.addThinkingStep('🔧 运行命令');
      speakStatus({ kind: 'running-command' });
    });
    unlisten = await Promise.all([
      listen('chat-thinking-start', () => {
        characterState.toThinking();
        chatStore.addThinkingStep('🤔 正在思考...');
        cancelPendingStatusSpeech();
        pendingStatusSpeech = setTimeout(() => speakStatus({ kind: 'thinking' }), 250);
        debugState('chat-thinking-start');
      }),
      listen<{ emotion: string; total_chars: number; has_audio: boolean }>('chat-speaking-start', (e) => {
        cancelPendingStatusSpeech();
        debugState('chat-speaking-start', e.payload as unknown as Record<string, unknown>);
        lastSpeakingHadAudio = e.payload.has_audio;
        if (!$chatStore.isStreaming) {
          chatStore.startStream();
        }
        chatStore.startTypewriter(e.payload.emotion);
        if (!e.payload.has_audio && !$layoutStore.expanded) {
          layoutStore.toggle();
        }
        if (conversationActive && e.payload.has_audio) {
          invoke('notify_conversation_tts_started').catch(console.error);
        }
      }),
      listen('chat-audio-playing', () => {
        cancelPendingStatusSpeech();
        lastSpokenStatus = null;
        debugState('chat-audio-playing');
        islandMode = 'idle';
        characterState.toSpeaking();
        chatStore.clearThinkingSteps();
        spiritPhase = 3;
      }),
      listen<{ delta: string }>('chat-stream', (e) => {
        cancelPendingStatusSpeech();
        if (!firstStreamDelta) debugState('chat-stream:first-delta', { deltaPreview: e.payload.delta.slice(0, 40) });
        chatStore.appendDelta(e.payload.delta);
        if (!firstStreamDelta) {
          firstStreamDelta = true;
          spiritPhase = Math.max(spiritPhase, 2);
        }
      }),
      listen('chat-stream-end', () => {
        cancelPendingStatusSpeech();
        lastSpokenStatus = null;
        debugState('chat-stream-end');
        islandMode = 'idle';
        chatStore.endStream();
        // If audio is expected, let chat-audio-playing / chat-audio-done drive the transition.
        if (!lastSpeakingHadAudio && get(characterState) !== 'speaking') {
          characterState.toIdle();
          spiritPhase = 0;
        }
        autoCloseBreakConfirm();
      }),
      listen('chat-audio-done', () => {
        debugState('chat-audio-done');
        chatStore.endStream();
        if (conversationActive) {
          // Hand control back to the conversation worker; it drives characterState via
          // conversation-state events. Skip the toIdle transition here.
          invoke('notify_conversation_tts_done').catch(console.error);
          spiritPhase = 0;
        } else {
          characterState.transition('speaking', 'idle');
          spiritPhase = 0;
        }
        autoCloseBreakConfirm();
      }),

      // LLM intermediate thinking/reasoning updates (in-place update of last 🤔 step)
      listen<string>('chat-thinking', (e) => {
        chatStore.updateLastThinkingStep(e.payload);
      }),

      // Tool call start notification
      listen<string>('chat-tool-call', (e) => {
        cancelPendingStatusSpeech();
        try {
          const payload = JSON.parse(e.payload);
          // Clean up tool name: strip common prefixes
          let toolName = payload.name
            .replace(/^mcp_tradingview_/, '')
            .replace(/^mcp_/, '')
            .replace(/_/g, ' ')
            .trim();
          chatStore.addThinkingStep(`🔧 查询 ${toolName}...`);
          if (toolName) speakStatus({ kind: 'querying', name: toolName });
        } catch {
          chatStore.addThinkingStep(`🔧 正在执行操作...`);
          speakStatus({ kind: 'executing' });
        }
      }),
      listen<string>('chat-stream-error', (e) => {
        cancelPendingStatusSpeech();
        lastSpokenStatus = null;
        debugState('chat-stream-error', { error: e.payload });
        islandMode = 'idle';
        chatStore.setError(e.payload);
        characterState.toIdle();
        spiritPhase = 0;
        autoCloseBreakConfirm();
      }),

      listen('bridge-thinking-start', () => {
        debugState('bridge-thinking-start');
        startBridgeThinking();
      }),
      listen('bridge-push-received', () => {
        debugState('bridge-push-received');
        // Full cleanup: clear timer + all UI state (same as stopBridgeThinking but silent)
        clearBridgeFallbackTimer();
        if (!bridgeThinkingActive) return;
        bridgeThinkingActive = false;
        islandMode = 'idle';
        spiritPhase = 0;
        cancelPendingStatusSpeech();
        lastSpokenStatus = null;
        chatStore.clearThinkingSteps();
        characterState.toIdle();
      }),

      // Bridge-mode intermediate events (reasoning + tool calls from Hermes via bridge)
      listen<string>("bridge-thinking", (e) => {
        chatStore.updateLastThinkingStep(e.payload);
      }),
      listen<string>("bridge-tool-call", (e) => {
        cancelPendingStatusSpeech();
        try {
          const payload = JSON.parse(e.payload);
          let toolName = payload.name
            .replace(/^mcp_tradingview_/, "")
            .replace(/^mcp_/, "")
            .replace(/_/g, " ")
            .trim();
          chatStore.addThinkingStep(`🔧 查询 ${toolName}...`);
          if (toolName) speakStatus({ kind: 'querying', name: toolName });
        } catch {
          chatStore.addThinkingStep("🔧 正在执行操作...");
          speakStatus({ kind: 'executing' });
        }
      }),
      listen('bridge-turn-finished', () => {
        debugState('bridge-turn-finished');
        if (bridgeThinkingActive) {
          stopBridgeThinking('暂时还没有收到 Hermes 回推');
        }
        autoCloseBreakConfirm();
      }),
      listen<string>('bridge-turn-error', (e) => {
        debugState('bridge-turn-error', { error: e.payload });
        stopBridgeThinking(`Hermes 转发失败: ${e.payload}`);
        autoCloseBreakConfirm();
      }),

      listen('fn-key-down', () => {
        debugState('fn-key-down');
        const cfg = get(settingsStore);
        const setupState = get(chatStore).voiceSetupState;
        if (setupState === 'installing' || setupState === 'error') {
          chatStore.setError(convLabels(cfg.tts_primary_voice).voiceSetupNotReady);
          return;
        }
        if (cfg.continuous_conversation) {
          if (conversationActive) {
            invoke('stop_continuous_conversation').catch(console.error);
            conversationActive = false;
            chatStore.setVoiceStatus(null);
            islandMode = 'idle';
            spiritPhase = 0;
            characterState.toIdle();
            return;
          }
          requestNewTurn(() => {
            conversationActive = true;
            islandMode = 'recording';
            spiritPhase = 0;
            firstStreamDelta = false;
            chatStore.clear();
            chatStore.setVoiceStatus(voiceListeningText());
            characterState.toListening();
            invoke('start_continuous_conversation', {
              silenceTimeoutSecs: cfg.silence_timeout_secs,
              pauseToleranceMs: cfg.pause_tolerance_ms,
              speechRmsThreshold: cfg.speech_rms_threshold,
            })
              .catch((e) => {
                console.error('[continuous] start failed', e);
                conversationActive = false;
                characterState.toIdle();
                islandMode = 'idle';
                chatStore.setError(`连续对话启动失败: ${e}`);
              });
          });
          return;
        }
        requestNewTurn(() => {
          islandMode = 'recording';
          spiritPhase = 0;
          firstStreamDelta = false;
          characterState.toListening();
          chatStore.clear();
          conversationActive = true;
          invoke('start_continuous_conversation', {
            silenceTimeoutSecs: cfg.silence_timeout_secs,
            pauseToleranceMs: cfg.pause_tolerance_ms,
            speechRmsThreshold: cfg.speech_rms_threshold,
            singleShot: true,
          }).catch((e) => {
            console.error('[single-shot] start failed', e);
            conversationActive = false;
            characterState.toIdle();
            islandMode = 'idle';
            chatStore.setError(`录音启动失败: ${e}`);
          });
        });
      }),
      listen('fn-key-up', () => {
        debugState('fn-key-up');
        if (get(settingsStore).continuous_conversation) {
          // In continuous mode, hotkey is a toggle on key-down; key-up is a no-op.
          return;
        }
        islandMode = 'thinking';
        spiritPhase = 1;
        characterState.toThinking();
        if (audioLevelTimer) { clearInterval(audioLevelTimer); audioLevelTimer = null; }
        invoke('stop_voice_recording').then(() => {
          // Re-arm wake listener after single-shot recording stops.
          // The capture device is now free, and the rest (STT→LLM→TTS)
          // doesn't use the mic.
          armWakeListenerIfEnabled();
        }).catch((e) => {
          console.warn('[stop_voice_recording]', e);
          islandMode = 'idle';
          spiritPhase = 0;
          characterState.toIdle();
          chatStore.setError(`录音停止失败: ${e}`);
        });
      }),
      listen('voice-cancel', () => {
        debugState('voice-cancel');
        islandMode = 'idle';
        spiritPhase = 0;
        characterState.toIdle();
        if (audioLevelTimer) { clearInterval(audioLevelTimer); audioLevelTimer = null; }
        invoke('cancel_voice_recording').catch(console.error);
      }),
      listen<{ text: string; language: string }>('stt-result', (e) => {
        debugState('stt-result', { language: e.payload.language, len: e.payload.text.length });
        islandMode = 'thinking';
        if (e.payload.text.trim()) {
          handleSendMessage(e.payload.text, e.payload.language);
        } else if (conversationActive) {
          // Empty result in continuous mode: worker stays Listening, just ignore.
          islandMode = 'recording';
        } else {
          spiritPhase = 0;
          characterState.toIdle();
          chatStore.setError('语音识别结果为空，请重试');
          if (audioLevelTimer) { clearInterval(audioLevelTimer); audioLevelTimer = null; }
        }
      }),
      listen<{ error: string }>('stt-error', (e) => {
        debugState('stt-error', { error: e.payload.error });
        islandMode = 'idle';
        spiritPhase = 0;
        console.warn('[STT]', e.payload.error);
        chatStore.setError(`语音识别失败: ${e.payload.error}`);
        characterState.toIdle();
        if (audioLevelTimer) { clearInterval(audioLevelTimer); audioLevelTimer = null; }
      }),

      // ─── Continuous conversation events ───
      listen<string>('conversation-state', (e) => {
        const s = e.payload as 'listening' | 'transcribing' | 'speaking';
        debugState('conversation-state', { state: s });
        if (!conversationActive) return;
        if (s === 'listening') {
          chatStore.setVoiceStatus(voiceListeningText());
          characterState.toListening();
          islandMode = 'recording';
          spiritPhase = 0;
        } else if (s === 'transcribing') {
          chatStore.setVoiceStatus(null);
          characterState.toThinking();
          islandMode = 'thinking';
          spiritPhase = 1;
        } else if (s === 'speaking') {
          // Transition to speaking visuals is driven by chat-audio-playing, not by
          // backend Speaking state — the backend flips to Speaking right after the STT
          // result returns (before TTS actually plays), which would otherwise skip the
          // thinking animation. Only clear the voice-status row here.
          chatStore.setVoiceStatus(null);
        }
      }),
      listen('conversation-ended', () => {
        debugState('conversation-ended');
        conversationActive = false;
        chatStore.setVoiceStatus(null);
        islandMode = 'idle';
        spiritPhase = 0;
        characterState.toIdle();
        armWakeListenerIfEnabled();
      }),

      // ─── Wake-word linkage (FEAT-C4) ───
      // Rust emits fn-key-down directly on wake match, so all hotkey logic
      // (single-shot / continuous / owner transitions) is reused automatically.
      // This listener is kept for debug logging only.
      listen<{ score: number }>('wake-word-detected', (e) => {
        debugState('wake-word-detected', { score: e.payload.score });
      }),
      listen<{ error: string }>('wake-listener-error', (e) => {
        console.warn('[wake] listener error:', e.payload.error);
      }),
      listen('conversation-barge-in', () => {
        debugState('conversation-barge-in');
        // TTS was cut; clear in-flight UI so the new utterance can render cleanly.
        chatStore.endStream();
        chatStore.clearThinkingSteps();
        chatStore.setVoiceStatus(voiceListeningText());
        spiritPhase = 0;
      }),

      // ─── First-launch PA venv bootstrap (~/.pocket-agent/venv) ───
      listen('venv-setup-ready', () => {
        chatStore.setVoiceSetup({ voiceSetupState: 'ready', voiceSetupPhase: '', voiceSetupDetail: '' });
      }),
      listen('venv-setup-started', () => {
        chatStore.setVoiceSetup({ voiceSetupState: 'installing', voiceSetupPhase: '', voiceSetupDetail: '' });
      }),
      listen<{ phase: string; detail: string }>('venv-setup-progress', (e) => {
        chatStore.setVoiceSetup({
          voiceSetupState: 'installing',
          voiceSetupPhase: e.payload.phase,
          voiceSetupDetail: e.payload.detail ?? '',
        });
      }),
      listen('venv-setup-done', () => {
        chatStore.setVoiceSetup({ voiceSetupState: 'ready', voiceSetupPhase: '', voiceSetupDetail: '' });
      }),
      listen<{ phase: string; message: string }>('venv-setup-error', (e) => {
        chatStore.setVoiceSetup({
          voiceSetupState: 'error',
          voiceSetupPhase: e.payload.phase,
          voiceSetupDetail: e.payload.message ?? '',
        });
      }),

      listen('accessibility-permission-required', () => {
        showAccessibilityGuide = true;
      }),
      listen('tray-open-settings', () => {
        openSettings();
      }),
      listen('tray-open-history', () => {
        openChatHistory();
      }),

      // API push: external message pushed to PA (e.g. from Hermes cron)
      // Only call speak_text — text display is handled by the speak_text
      // Rust side which emits chat-speaking-start (typewriter) + chat-stream (delta).
      listen<{ text: string; emotion: string; voice: string | null }>("api-push", (e) => {
        debugState('api-push', { emotion: e.payload.emotion, hasVoice: !!e.payload.voice, len: e.payload.text.length });
        const { text, emotion, voice } = e.payload;
        if (!text.trim()) return;
        currentInputLang = voice ? langFromVoice(voice) : detectLang(text);
        clearBridgeFallbackTimer();
        bridgeThinkingActive = false;
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

  // Track wake_word_enabled across settings changes so toggling the switch in
  // SettingsPanel arms / disarms immediately, without requiring a PA restart.
  let prevWakeEnabled = false;
  let unsubSettings: (() => void) | null = null;

  onMount(async () => {
    await settingsStore.load();
    // Apply double-click mode setting to hotkey listener
    const s = $settingsStore;
    if (s.double_click_to_record) {
      await invoke('set_double_click_mode', { enabled: true });
    }
    await restoreWindowPosition();
    await setupListeners();
    await setupWindowPositionSave();
    prevWakeEnabled = s.wake_word_enabled;
    unsubSettings = settingsStore.subscribe((cur) => {
      if (cur.wake_word_enabled === prevWakeEnabled) return;
      prevWakeEnabled = cur.wake_word_enabled;
      if (cur.wake_word_enabled) {
        armWakeListenerIfEnabled();
      } else {
        disarmWakeListener();
      }
    });
    await armWakeListenerIfEnabled();
  });

  onDestroy(() => {
    unlisten.forEach((fn) => fn());
    if (audioLevelTimer) clearInterval(audioLevelTimer);
    clearBridgeFallbackTimer();
    if (unsubSettings) unsubSettings();
    if (conversationActive) {
      invoke('stop_continuous_conversation').catch(() => {});
    }
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
  <div class="main-row">
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
      <DynamicIsland mode={islandMode} audioLevel={audioLevel} />
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
  </div>

  <!-- Status panel: spans avatar + chat width, only when expanded -->
  {#if $layoutStore.expanded}
    <div class="status-row"><StatusPanel /></div>
  {/if}


  <!-- Settings panel (takes over window when open) -->
  {#if showSettings}
    <SettingsPanel bind:visible={showSettings} onclose={closeSettings} />
  {/if}

  <!-- Break confirm modal: shown when user triggers a new turn while one is in progress -->
  <BreakConfirmModal
    visible={showBreakConfirm}
    onbreak={confirmBreak}
    oncancel={cancelBreak}
  />

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
    flex-direction: column;
    align-items: stretch;
    background: transparent;
    position: relative;
  }

  .main-row {
    display: flex;
    flex-direction: row;
    align-items: center;
    width: 100%;
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
  .status-row { margin-top: 6px; width: 100%; flex-shrink: 0; }
</style>
