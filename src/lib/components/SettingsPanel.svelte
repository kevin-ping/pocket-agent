<script lang="ts">
  import { fly } from 'svelte/transition';
  import { tick } from 'svelte';
  import { settingsStore, type AppSettings } from '../stores/settings';
  import { t, convLabels } from '../i18n';
  import { invoke } from '@tauri-apps/api/core';

  const VOICE_OPTIONS = [
    { group: '🇨🇳 中文', voices: [
      { id: 'zh-CN-XiaoxiaoNeural', label: '晓晓（女·温暖）' },
      { id: 'zh-CN-XiaoyiNeural',   label: '晓伊（女·活泼）' },
      { id: 'zh-CN-YunxiNeural',    label: '云希（男·阳光）' },
      { id: 'zh-CN-YunjianNeural',  label: '云健（男·激情）' },
      { id: 'zh-CN-YunyangNeural',  label: '云扬（男·专业）' },
      { id: 'zh-CN-liaoning-XiaobeiNeural', label: '小北（女·东北话）' },
      { id: 'zh-CN-shaanxi-XiaoniNeural',   label: '小妮（女·陕西话）' },
      { id: 'zh-HK-HiuGaaiNeural', label: '曉佳（女·粵語）' },
      { id: 'zh-HK-WanLungNeural', label: '雲龍（男·粵語）' },
      { id: 'zh-TW-HsiaoChenNeural', label: '曉臻（女·台灣）' },
      { id: 'zh-TW-YunJheNeural',    label: '宥哲（男·台灣）' },
    ]},
    { group: '🇯🇵 日本語', voices: [
      { id: 'ja-JP-NanamiNeural', label: 'ななみ（女）' },
      { id: 'ja-JP-KeitaNeural',  label: 'けいた（男）' },
    ]},
    { group: '🇺🇸 English (US)', voices: [
      { id: 'en-US-AvaNeural',    label: 'Ava（女）' },
      { id: 'en-US-AndrewNeural', label: 'Andrew（男）' },
      { id: 'en-US-JennyNeural',  label: 'Jenny（女）' },
    ]},
    { group: '🇬🇧 English (UK)', voices: [
      { id: 'en-GB-SoniaNeural',  label: 'Sonia（女）' },
      { id: 'en-GB-LibbyNeural',  label: 'Libby（女）' },
      { id: 'en-GB-MaisieNeural', label: 'Maisie（女）' },
      { id: 'en-GB-RyanNeural',   label: 'Ryan（男）' },
      { id: 'en-GB-ThomasNeural', label: 'Thomas（男）' },
    ]},
    { group: '🇰🇷 한국어', voices: [
      { id: 'ko-KR-SunHiNeural',  label: '선히（女）' },
      { id: 'ko-KR-InJoonNeural', label: '인준（男）' },
    ]},
    { group: '🇫🇷 Français', voices: [
      { id: 'fr-FR-DeniseNeural', label: 'Denise（女）' },
      { id: 'fr-FR-HenriNeural',  label: 'Henri（男）' },
    ]},
    { group: '🇩🇪 Deutsch', voices: [
      { id: 'de-DE-KatjaNeural',   label: 'Katja（女）' },
      { id: 'de-DE-ConradNeural',  label: 'Conrad（男）' },
    ]},
    { group: '🇪🇸 Español', voices: [
      { id: 'es-ES-ElviraNeural', label: 'Elvira（女）' },
      { id: 'es-ES-AlvaroNeural', label: 'Alvaro（男）' },
    ]},
  ];

  // Runes mode props
  let { visible = $bindable(false), onclose }: {
    visible?: boolean;
    onclose?: () => void;
  } = $props();

  // Reactive local state
  let local = $state<AppSettings>({ ...$settingsStore });


  // Initialize local state from store when panel opens
  // Only sync on initial open, not continuously
  let panelWasVisible = false;
  $effect(() => {
    if (visible && !panelWasVisible) {
      // Panel just opened: load from store
      local = { ...$settingsStore };
      void refreshVariantCount();
      panelWasVisible = true;
    } else if (!visible && panelWasVisible) {
      // Panel just closed: reset flag
      panelWasVisible = false;
    }
  });

  // Avatar upload
  let fileInput = $state<HTMLInputElement>(undefined!);

  function triggerAvatarUpload() {
    fileInput?.click();
  }

  function handleAvatarFile(e: Event) {
    const file = (e.target as HTMLInputElement).files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      local.avatar_image = reader.result as string;
    };
    reader.readAsDataURL(file);
  }

  function removeAvatar() {
    local.avatar_image = null;
    if (fileInput) fileInput.value = '';
  }

  let gifFileInput = $state<HTMLInputElement>(undefined!);

  function triggerAvatarGifUpload() {
    gifFileInput?.click();
  }

  function handleAvatarGifFile(e: Event) {
    const file = (e.target as HTMLInputElement).files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      local.avatar_gif = reader.result as string;
    };
    reader.readAsDataURL(file);
  }

  function removeAvatarGif() {
    local.avatar_gif = null;
    if (gifFileInput) gifFileInput.value = '';
  }

  let capturing = $state(false);
  let applying = $state(false);

  let captureTimeout: ReturnType<typeof setTimeout> | null = null;
  let capturePollTimer: ReturnType<typeof setInterval> | null = null;

  // Speaker-print enrollment state
  const ENROLL_DURATION_S = 3;
  let enrolling = $state(false);
  let enrollCountdown = $state(0);
  let enrollMessage = $state<string | null>(null);
  let enrollNameDialog = $state(false);
  let enrollNameInput = $state('');
  let enrollTimer: ReturnType<typeof setInterval> | null = null;

  // Training mode: repeated enroll rounds to collect whisper variants
    let trainRound = $state(0);
  let trainVariantCount = $state(0);

  async function refreshVariantCount() {
    const name = $settingsStore.last_enrolled_speaker;
    if (!name) { trainVariantCount = 0; return; }
    try {
      trainVariantCount = await invoke<number>('get_wake_variant_count', { name });
    } catch {
      trainVariantCount = 0;
    }
  }

  // Click handler for the main "录制唤醒词" button. If a previous speaker name
  // has been saved, reuse it silently (single-user workflow — no dialog).
  // Otherwise prompt for a name.
  function onEnrollClick() {
    enrollMessage = null;
    const saved = $settingsStore.last_enrolled_speaker;
    if (saved && /^[A-Za-z0-9_-]{1,32}$/.test(saved)) {
      void runEnrollFlow(saved);
    } else {
      enrollNameInput = 'Me';
      enrollNameDialog = true;
    }
  }

  // Explicit rename entry — always shows the dialog, pre-filled with the
  // current saved name so the user can edit it.
  function openRenameDialog() {
    enrollMessage = null;
    enrollNameInput = $settingsStore.last_enrolled_speaker ?? '';
    enrollNameDialog = true;
  }

  function cancelEnrollDialog() {
    enrollNameDialog = false;
  }

  async function confirmEnrollName() {
    const name = enrollNameInput.trim();
    if (!/^[A-Za-z0-9_-]{1,32}$/.test(name)) {
      enrollMessage = '名字仅允许 字母/数字/_/-（1-32 位）';
      return;
    }
    enrollNameDialog = false;
    await runEnrollFlow(name);
  }

  async function runEnrollFlow(name: string) {
    enrolling = true;
    enrollMessage = null;
    enrollCountdown = ENROLL_DURATION_S;
    try {
      await invoke('start_enroll_recording');
    } catch (e) {
      enrolling = false;
      enrollMessage = `启动录音失败: ${String(e)}`;
      return;
    }
    enrollTimer = setInterval(() => {
      enrollCountdown -= 1;
      if (enrollCountdown <= 0) {
        if (enrollTimer) { clearInterval(enrollTimer); enrollTimer = null; }
        void finishEnroll(name);
      }
    }, 1000);
  }

  async function finishEnroll(name: string) {
    try {
      const wavPath = await invoke<string>('stop_enroll_recording');
      const result = await invoke<{ ok: boolean; speaker_id: string; embedding_dim: number; snr_db: number }>(
        'enroll_speaker',
        { name, audioPath: wavPath }
      );
      if (result.ok) {
        await refreshVariantCount();
        enrollMessage = convLabels($settingsStore.tts_primary_voice).enrollSuccess(trainVariantCount);
        // Persist the speaker name so the next enrollment skips the name dialog.
        if ($settingsStore.last_enrolled_speaker !== name) {
          void settingsStore.save({ last_enrolled_speaker: name });
        }
        // Re-arm wake listener after enrollment (it was stopped for recording).
        setTimeout(() => {
          if ($settingsStore.wake_word_enabled) {
            invoke('start_wake_word_listening', { threshold: $settingsStore.wake_word_threshold }).catch(() => {});
          }
        }, 1000);
      } else {
        enrollMessage = '注册失败';
      }
    } catch (e) {
      const msg = String(e ?? '');
      if (msg.includes('too_quiet')) {
        const m = msg.match(/"rms_dbfs"\s*:\s*(-?\d+(?:\.\d+)?)/);
        const dbfs = m ? parseFloat(m[1]) : undefined;
        enrollMessage = convLabels($settingsStore.tts_primary_voice).enrollFailedTooQuiet(dbfs);
      } else if (msg.includes('录音时间太短')) {
        enrollMessage = '录音时间太短，请重试';
      } else {
        enrollMessage = `注册失败: ${msg}`;
      }
    } finally {
      enrolling = false;
    }
  }

  async function startTraining() {
    const saved = $settingsStore.last_enrolled_speaker;
    if (!saved || !/^[A-Za-z0-9_-]{1,32}$/.test(saved)) {
      enrollMessage = '请先录制一次唤醒词';
      return;
    }
    // Stop wake listener to free capture device
    try { await invoke('stop_wake_word_listening'); } catch {}
    trainRound++;
    enrolling = true;
    enrollMessage = null;
    enrollCountdown = ENROLL_DURATION_S;
    try {
      await invoke('start_enroll_recording');
    } catch (e) {
      enrolling = false;
      enrollMessage = `录音失败: ${String(e)}`;
      return;
    }
    enrollTimer = setInterval(() => {
      enrollCountdown -= 1;
      if (enrollCountdown <= 0) {
        if (enrollTimer) { clearInterval(enrollTimer); enrollTimer = null; }
        void finishTrainRound(saved);
      }
    }, 1000);
  }

  async function finishTrainRound(name: string) {
    try {
      const wavPath = await invoke<string>('stop_enroll_recording');
      const result = await invoke<{ ok: boolean; speaker_id: string }>(
        'train_speaker',
        { name, audioPath: wavPath }
      );
      if (result.ok) {
        await refreshVariantCount();
        enrollMessage = `训练完成！共 ${trainVariantCount} 个变体`;
        if ($settingsStore.last_enrolled_speaker !== name) {
          void settingsStore.save({ last_enrolled_speaker: name });
        }
      } else {
        enrollMessage = `训练失败`;
      }
    } catch (e) {
      enrollMessage = `训练出错: ${String(e)}`;
    } finally {
      enrolling = false;
      // Re-arm wake listener
      setTimeout(() => {
        if ($settingsStore.wake_word_enabled) {
          invoke('start_wake_word_listening', { threshold: $settingsStore.wake_word_threshold }).catch(() => {});
        }
      }, 1000);
    }
  }

  function cleanupCapture() {
    if (capturePollTimer) { clearInterval(capturePollTimer); capturePollTimer = null; }
    if (captureTimeout) { clearTimeout(captureTimeout); captureTimeout = null; }
  }

  async function startCapture() {
    capturing = true;
    applying = false;
    saveError = '';
    // Triple-guarantee rendering chain before invoke:
    await tick();
    await new Promise(r => requestAnimationFrame(r));
    await new Promise(r => setTimeout(r, 50));
    try {
      await invoke('start_capture');
    } catch (e) {
      console.warn('[hotkey] start_capture failed:', e);
      if (String(e).includes('accessibility')) {
        saveError = '需要辅助功能权限：系统设置 → 隐私与安全性 → 辅助功能 → 允许此应用';
      } else {
        saveError = '按键捕获失败，请重试';
      }
      capturing = false;
      return;
    }
    // Poll every 80ms — reads an AtomicI64 on Rust side (sub-microsecond)
    capturePollTimer = setInterval(async () => {
      try {
        const result = await invoke<[number, string] | null>('poll_capture');
        if (result) {
          cleanupCapture();
          // Immediately show yellow "applying" state with the captured key name
          capturing = false;
          applying = true;
          local.hotkey_code = result[0];
          local.hotkey_name = result[1];
          // Background: update backend + persist
          invoke('update_hotkey', { code: result[0] });
          settingsStore.save({ hotkey_code: result[0], hotkey_name: result[1] }).then(() => {
            applying = false;
          });
        }
      } catch {
        // poll failed — keep trying until timeout
      }
    }, 80);
    // Timeout safeguard
    captureTimeout = setTimeout(() => {
      cleanupCapture();
      if (capturing) {
        capturing = false;
        saveError = '按键捕获超时，请重新尝试';
      }
    }, 6000);
  }

  let saveError = $state('');

  async function save() {
    saveError = '';
    try {
      await settingsStore.save(local);
      visible = false;
      onclose?.();
    } catch (e) {
      saveError = '保存失败，请重试';
      console.error('[settings] save error:', e);
    }
  }

  function cancel() {
    visible = false;
    onclose?.();
  }

  let volumePct = $derived(Math.round(local.volume * 100));
</script>
{#if visible}
  <!-- Full-window overlay; App.svelte resizes the Tauri window to 380×500 before showing this -->
  <div
    class="panel"
    in:fly={{ y: 18, duration: 200, opacity: 0 }}
    out:fly={{ y: 12, duration: 140, opacity: 0 }}
    role="dialog"
    aria-label={t($settingsStore.tts_primary_voice).ariaSettings}
  >
    <!-- Header -->
    <div class="header">
      <button class="close-btn" onclick={cancel} aria-label={t($settingsStore.tts_primary_voice).ariaCloseSettings}></button>
      <span class="header-title">{t($settingsStore.tts_primary_voice).settings}</span>
    </div>

    <!-- Scrollable body -->
    <div class="body">

      <!-- ── Avatar section ── -->
      <div class="section-label">{t($settingsStore.tts_primary_voice).avatar}</div>
      <div class="avatar-section">
        <!-- Avatar preview / upload trigger -->
        <button class="avatar-preview" onclick={triggerAvatarUpload} title={t($settingsStore.tts_primary_voice).clickToUpload}>
          {#if local.avatar_image}
            <img src={local.avatar_image} alt={t($settingsStore.tts_primary_voice).ariaAvatar} class="avatar-img" />
          {:else}
            <div class="avatar-placeholder">
              <div class="placeholder-face">
                <div class="placeholder-eye l"></div>
                <div class="placeholder-eye r"></div>
                <div class="placeholder-mouth"></div>
              </div>
            </div>
          {/if}
          <div class="avatar-overlay">{t($settingsStore.tts_primary_voice).upload}</div>
        </button>
        <div class="avatar-info">
          <p class="avatar-hint">{t($settingsStore.tts_primary_voice).supportedFormats}</p>
          {#if local.avatar_image}
            <button class="remove-btn" onclick={removeAvatar}>{t($settingsStore.tts_primary_voice).removeAvatar}</button>
          {:else}
            <p class="avatar-hint muted">{t($settingsStore.tts_primary_voice).defaultAvatarHint}</p>
          {/if}
        </div>
        <input
          bind:this={fileInput}
          type="file"
          accept="image/*"
          style="display:none"
          onchange={handleAvatarFile}
        />
        <input
          bind:this={gifFileInput}
          type="file"
          accept="image/gif"
          style="display:none"
          onchange={handleAvatarGifFile}
        />
        {#if local.avatar_image}
          <div class="avatar-gif-section">
            <div class="section-label" style="margin-top:4px">动画头像 (thinking/speaking)</div>
            <div style="display:flex;align-items:center;gap:6px">
              {#if local.avatar_gif}
                <img src={local.avatar_gif} alt="GIF" style="width:32px;height:32px;border-radius:50%;object-fit:cover" />
                <button class="remove-btn" onclick={removeAvatarGif}>✕</button>
              {:else}
                <button class="remove-btn" style="opacity:0.7" onclick={triggerAvatarGifUpload}>上传 GIF</button>
              {/if}
            </div>
          </div>
        {/if}
      </div>



      <!-- ── Hotkey section ── -->
      <div class="section-label">Hotkey</div>
      <div class="field-row">
        <span class="field-label">Record Key</span>
        {#if capturing}
          <button class="capture-btn active" disabled>按下快捷键...</button>
        {:else if applying}
          <button class="capture-btn applying" disabled>按键更换中...</button>
        {:else}
          <button class="capture-btn" onclick={startCapture}>{local.hotkey_name || $settingsStore.hotkey_name || 'RightShift'}</button>
        {/if}
      </div>

      <div class="field-row">
        <span class="field-label">Double-Click Record</span>
        <div class="toggle-wrap">
          <input type="checkbox" id="double-click-record" class="toggle-input" bind:checked={local.double_click_to_record} />
          <label for="double-click-record" class="toggle-track">
            <span class="toggle-thumb"></span>
          </label>
        </div>
      </div>

      <div class="field-row">
        <span class="field-label">Voice Output</span>
        <div class="toggle-wrap">
          <input type="checkbox" id="tts-enabled" class="toggle-input" bind:checked={local.tts_enabled} />
          <label for="tts-enabled" class="toggle-track">
            <span class="toggle-thumb"></span>
          </label>
        </div>
      </div>

      <!-- ── Wake word + wake-word sample verification ── -->
      <div class="field-row">
        <span class="field-label">{convLabels($settingsStore.tts_primary_voice).wakeWordLabel}</span>
        <div class="toggle-wrap">
          <input type="checkbox" id="wake-word-enabled" class="toggle-input" bind:checked={local.wake_word_enabled} />
          <label for="wake-word-enabled" class="toggle-track">
            <span class="toggle-thumb"></span>
          </label>
        </div>
      </div>

      {#if local.wake_word_enabled}
        <div class="field-row">
          <span class="field-label">
            {convLabels($settingsStore.tts_primary_voice).wakeWordThreshold}
            <span class="volume-pct">{local.wake_word_threshold.toFixed(2)}</span>
          </span>
          <input
            class="field-slider"
            type="range"
            min="0.30"
            max="0.90"
            step="0.05"
            bind:value={local.wake_word_threshold}
            aria-label={convLabels($settingsStore.tts_primary_voice).wakeWordThreshold}
          />
        </div>

        <div class="field-row">
          <span class="field-label">{convLabels($settingsStore.tts_primary_voice).speakerVerificationLabel}</span>
          <div class="toggle-wrap">
            <input type="checkbox" id="speaker-verification-enabled" class="toggle-input" bind:checked={local.speaker_verification_enabled} />
            <label for="speaker-verification-enabled" class="toggle-track">
              <span class="toggle-thumb"></span>
            </label>
          </div>
        </div>

        {#if local.speaker_verification_enabled}
          <div class="field-row enroll-row">
            <span class="field-label" style="min-width: 0; flex: 1;">
              <span style="font-size: 11px; color: rgba(232, 232, 240, 0.45);">
                {#if enrollMessage}
                  {enrollMessage}
                {:else if enrolling}
                  {convLabels($settingsStore.tts_primary_voice).enrollPhraseHint}
                {:else}
                  {convLabels($settingsStore.tts_primary_voice).speakerVerificationHint}
                {/if}
              </span>
              {#if !enrolling && $settingsStore.last_enrolled_speaker}
                <button type="button" class="enroll-rename-link" onclick={openRenameDialog}>
                  {convLabels($settingsStore.tts_primary_voice).enrollRenameAction($settingsStore.last_enrolled_speaker)}
                </button>
              {/if}
            </span>
            {#if enrolling}
              <button class="capture-btn active" disabled>
                {convLabels($settingsStore.tts_primary_voice).enrollRecordingCountdown(enrollCountdown)}
              </button>
            {:else}
              <button class="capture-btn" onclick={onEnrollClick}>
                {convLabels($settingsStore.tts_primary_voice).enrollSpeakerButton}
              </button>
              {#if $settingsStore.last_enrolled_speaker}
                <button class="capture-btn" style="margin-left: 4px; background: rgba(52, 152, 219, 0.7);" onclick={startTraining}>
                  训练 ({trainVariantCount})
                </button>
              {/if}
            {/if}
          </div>
        {/if}
      {/if}

      <div class="field-row">
        <span class="field-label">{convLabels($settingsStore.tts_primary_voice).continuousMode}</span>
        <div class="toggle-wrap">
          <input type="checkbox" id="continuous-conversation" class="toggle-input" bind:checked={local.continuous_conversation} />
          <label for="continuous-conversation" class="toggle-track">
            <span class="toggle-thumb"></span>
          </label>
        </div>
      </div>

      {#if local.continuous_conversation}
        <div class="field-row">
          <span class="field-label">
            {convLabels($settingsStore.tts_primary_voice).silenceTimeout}
            <span class="volume-pct">{convLabels($settingsStore.tts_primary_voice).silenceSecondsSuffix(local.silence_timeout_secs)}</span>
          </span>
          <input
            class="field-slider"
            type="range"
            min="3"
            max="10"
            step="1"
            bind:value={local.silence_timeout_secs}
            aria-label={convLabels($settingsStore.tts_primary_voice).silenceTimeout}
          />
        </div>

        <div class="field-row">
          <span class="field-label">
            {convLabels($settingsStore.tts_primary_voice).pauseTolerance}
            <span class="volume-pct">{convLabels($settingsStore.tts_primary_voice).pauseToleranceMsSuffix(local.pause_tolerance_ms)}</span>
          </span>
          <input
            class="field-slider"
            type="range"
            min="500"
            max="5000"
            step="100"
            bind:value={local.pause_tolerance_ms}
            aria-label={convLabels($settingsStore.tts_primary_voice).pauseTolerance}
          />
        </div>

        <div class="field-row">
          <span class="field-label">
            {convLabels($settingsStore.tts_primary_voice).micSensitivity}
            <span class="volume-pct">{convLabels($settingsStore.tts_primary_voice).micSensitivitySuffix(local.speech_rms_threshold)}</span>
          </span>
          <input
            class="field-slider"
            type="range"
            min="0.003"
            max="0.020"
            step="0.001"
            bind:value={local.speech_rms_threshold}
            aria-label={convLabels($settingsStore.tts_primary_voice).micSensitivity}
          />
        </div>

        <div class="field-row">
          <span class="field-label">{convLabels($settingsStore.tts_primary_voice).skipInterruptConfirm}</span>
          <div class="toggle-wrap">
            <input type="checkbox" id="skip-interrupt-confirm" class="toggle-input" bind:checked={local.skip_interrupt_confirmation} />
            <label for="skip-interrupt-confirm" class="toggle-track">
              <span class="toggle-thumb"></span>
            </label>
          </div>
        </div>
      {/if}

      <!-- ── Appearance section ── -->
      <div class="section-label">{t($settingsStore.tts_primary_voice).appearance}</div>

      <div class="field-row">
        <label class="field-label" for="dialog-style">{t($settingsStore.tts_primary_voice).skin}</label>
        <select id="dialog-style" class="field-input" bind:value={local.dialog_style}>
          <option value="default">{t($settingsStore.tts_primary_voice).defaultOption}</option>
        </select>
      </div>

      <div class="field-row">
        <span class="field-label">{t($settingsStore.tts_primary_voice).volume} <span class="volume-pct">{volumePct}%</span></span>
        <input
          class="field-slider"
          type="range"
          min="0"
          max="1"
          step="0.05"
          bind:value={local.volume}
          aria-label={t($settingsStore.tts_primary_voice).ariaVolume}
        />
      </div>

      <!-- ── Voice section ── -->
      <div class="section-label">{t($settingsStore.tts_primary_voice).voice}</div>

      <div class="field-row">
        <label class="field-label" for="voice-primary">{t($settingsStore.tts_primary_voice).primaryLang}</label>
        <select id="voice-primary" class="field-input" bind:value={local.tts_primary_voice}>
          {#each VOICE_OPTIONS as group}
            <optgroup label={group.group}>
              {#each group.voices as v}
                <option value={v.id}>{v.label}</option>
              {/each}
            </optgroup>
          {/each}
        </select>
        <label class="fixed-lang-check">
          <input type="checkbox" checked={local.fixed_lang === 'primary'} onchange={() => { local.fixed_lang = local.fixed_lang === 'primary' ? '' : 'primary'; }} />
          {t($settingsStore.tts_primary_voice).fixedLang}
        </label>
      </div>

      <div class="field-row">
        <label class="field-label" for="voice-aux1">{t($settingsStore.tts_primary_voice).aux1Lang}</label>
        <select id="voice-aux1" class="field-input" bind:value={local.tts_aux1_voice}>
          <option value="">{t($settingsStore.tts_primary_voice).none}</option>
          {#each VOICE_OPTIONS as group}
            <optgroup label={group.group}>
              {#each group.voices as v}
                <option value={v.id}>{v.label}</option>
              {/each}
            </optgroup>
          {/each}
        </select>
        {#if local.tts_aux1_voice}
          <label class="fixed-lang-check">
            <input type="checkbox" checked={local.fixed_lang === 'aux1'} onchange={() => { local.fixed_lang = local.fixed_lang === 'aux1' ? '' : 'aux1'; }} />
            {t($settingsStore.tts_primary_voice).fixedLang}
          </label>
        {/if}
      </div>

      <div class="field-row">
        <label class="field-label" for="voice-aux2">{t($settingsStore.tts_primary_voice).aux2Lang}</label>
        <select id="voice-aux2" class="field-input" bind:value={local.tts_aux2_voice}>
          <option value="">{t($settingsStore.tts_primary_voice).none}</option>
          {#each VOICE_OPTIONS as group}
            <optgroup label={group.group}>
              {#each group.voices as v}
                <option value={v.id}>{v.label}</option>
              {/each}
            </optgroup>
          {/each}
        </select>
        {#if local.tts_aux2_voice}
          <label class="fixed-lang-check">
            <input type="checkbox" checked={local.fixed_lang === 'aux2'} onchange={() => { local.fixed_lang = local.fixed_lang === 'aux2' ? '' : 'aux2'; }} />
            {t($settingsStore.tts_primary_voice).fixedLang}
          </label>
        {/if}
      </div>

      <div class="field-row">
        <label class="field-label" for="tts-format">{t($settingsStore.tts_primary_voice).audioFormat}</label>
        <select id="tts-format" class="field-input" bind:value={local.tts_format}>
          <option value="wav">{t($settingsStore.tts_primary_voice).wavLossless}</option>
          <option value="mp3">{t($settingsStore.tts_primary_voice).mp3Compact}</option>
        </select>
      </div>

      <p class="hint">{t($settingsStore.tts_primary_voice).autoDetectHint}</p>

    </div>

    <!-- Footer actions -->
    <div class="footer">
      {#if saveError}
        <span class="save-error">{saveError}</span>
      {/if}
      <button class="btn" onclick={cancel}>{t($settingsStore.tts_primary_voice).cancel}</button>
      <button class="btn primary" onclick={save}>{t($settingsStore.tts_primary_voice).save}</button>
    </div>

    {#if enrollNameDialog}
      <div class="enroll-modal-backdrop" role="dialog" aria-modal="true">
        <div class="enroll-modal">
          <p class="enroll-modal-title">请输入说话人名字</p>
          <!-- svelte-ignore a11y_autofocus -->
          <input
            class="enroll-modal-input"
            type="text"
            bind:value={enrollNameInput}
            placeholder="alice"
            maxlength="32"
            autofocus
            onkeydown={(e) => {
              if (e.key === 'Enter') { e.preventDefault(); confirmEnrollName(); }
              else if (e.key === 'Escape') { e.preventDefault(); cancelEnrollDialog(); }
            }}
          />
          {#if enrollMessage}
            <p class="enroll-modal-error">{enrollMessage}</p>
          {/if}
          <div class="enroll-modal-actions">
            <button class="btn" onclick={cancelEnrollDialog}>取消</button>
            <button class="btn primary" onclick={confirmEnrollName}>开始录制</button>
          </div>
        </div>
      </div>
    {/if}
  </div>
{/if}
<style>
  /* ─── Panel (full-window overlay) ─── */
  .panel {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: rgba(12, 12, 22, 0.98);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  /* ─── Header ─── */
  .header {
    height: 38px;
    display: flex;
    align-items: center;
    padding: 0 12px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
    flex-shrink: 0;
  }

  .header-title {
    flex: 1;
    font-size: 12px;
    font-weight: 600;
    color: rgba(232, 232, 240, 0.6);
    text-align: center;
  }

  .close-btn {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: none;
    background: rgba(255, 95, 86, 0.9);
    cursor: pointer;
    padding: 0;
    flex-shrink: 0;
    transition: filter 0.1s, transform 0.1s;
  }
  .close-btn:hover {
    filter: brightness(1.3);
    transform: scale(1.15);
  }
  .close-btn:active {
    transform: scale(0.95);
  }

  /* ─── Body ─── */
  .body {
    flex: 1;
    overflow-y: auto;
    padding: 16px 20px 4px;
    display: flex;
    flex-direction: column;
    scrollbar-width: thin;
    scrollbar-color: rgba(124, 158, 255, 0.2) transparent;
  }
  .body::-webkit-scrollbar       { width: 3px; }
  .body::-webkit-scrollbar-track { background: transparent; }
  .body::-webkit-scrollbar-thumb { background: rgba(124, 158, 255, 0.2); border-radius: 2px; }

  /* ─── Section label ─── */
  .section-label {
    font-size: 13px;
    font-weight: 700;
    color: rgba(124, 158, 255, 0.7);
    padding-top: 18px;
    padding-bottom: 8px;
    margin-bottom: 4px;
  }
  .section-label:first-child { padding-top: 4px; }

  /* ─── Avatar section ─── */
  .avatar-section {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 12px 0 8px;
  }

  .avatar-preview {
    width: 68px;
    height: 68px;
    border-radius: 50%;
    border: 2px solid rgba(124, 158, 255, 0.3);
    background: rgba(107, 140, 255, 0.15);
    cursor: pointer;
    position: relative;
    overflow: hidden;
    flex-shrink: 0;
    padding: 0;
    transition: border-color 0.15s;
  }
  .avatar-preview:hover { border-color: rgba(124, 158, 255, 0.6); }
  .avatar-preview:hover .avatar-overlay { opacity: 1; }

  .avatar-img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    border-radius: 50%;
  }

  .avatar-placeholder {
    width: 100%;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    background: radial-gradient(circle at 38% 35%, #6b8cff, #3d5af1);
  }

  .placeholder-face {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding-top: 2px;
  }

  .placeholder-eye {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: rgba(10, 10, 30, 0.8);
    display: inline-block;
  }
  .placeholder-face .placeholder-eye.l,
  .placeholder-face .placeholder-eye.r {
    display: inline-block;
  }

  .placeholder-face {
    flex-direction: row;
    flex-wrap: wrap;
    justify-content: center;
    gap: 0;
  }

  .placeholder-face .placeholder-eye { margin: 0 4px; }
  .placeholder-mouth {
    width: 14px;
    height: 4px;
    border-radius: 0 0 7px 7px;
    background: rgba(10, 10, 30, 0.8);
    margin-top: 5px;
    flex-basis: 100%;
    margin-left: auto;
    margin-right: auto;
  }

  .avatar-overlay {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 11px;
    color: rgba(255, 255, 255, 0.9);
    font-weight: 600;
    opacity: 0;
    transition: opacity 0.15s;
    border-radius: 50%;
    letter-spacing: 0.04em;
  }

  .avatar-info {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .avatar-hint {
    margin: 0;
    font-size: 11px;
    color: rgba(232, 232, 240, 0.45);
    line-height: 1.5;
  }
  .avatar-hint.muted { color: rgba(232, 232, 240, 0.28); font-style: italic; }

  .remove-btn {
    padding: 4px 10px;
    background: rgba(255, 80, 80, 0.12);
    border: 1px solid rgba(255, 80, 80, 0.25);
    border-radius: 6px;
    color: rgba(255, 130, 130, 0.85);
    font-size: 11px;
    cursor: pointer;
    transition: background 0.1s;
    align-self: flex-start;
  }
  .remove-btn:hover { background: rgba(255, 80, 80, 0.22); }

  /* ─── Field rows ─── */
  .field-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 9px 0;
    border-bottom: 1px solid rgba(255, 255, 255, 0.04);
    gap: 12px;
  }
  .field-row:last-of-type { border-bottom: none; }

  .field-label {
    font-size: 13px;
    color: rgba(232, 232, 240, 0.78);
    flex-shrink: 0;
    min-width: 72px;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .volume-pct {
    font-size: 11px;
    color: rgba(124, 158, 255, 0.7);
    font-variant-numeric: tabular-nums;
  }

  /* ─── Inputs ─── */
  .field-input {
    width: 172px;
    flex-shrink: 0;
  }


  select.field-input {
    -webkit-appearance: none;
    appearance: none;
    background-color: rgba(255, 255, 255, 0.06);
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6' viewBox='0 0 10 6'%3E%3Cpath d='M0 0l5 6 5-6z' fill='rgba(160,160,200,0.5)'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 10px center;
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 8px;
    padding: 6px 28px 6px 10px;
    color: rgba(232, 232, 240, 0.9);
    font-size: 12px;
    outline: none;
    cursor: pointer;
    transition: border-color 0.12s;
  }
  select.field-input:focus { border-color: rgba(124, 158, 255, 0.5); }
  select.field-input option { background: #1a1a2e; color: rgba(232, 232, 240, 0.9); }
  select.field-input optgroup { font-weight: 700; color: rgba(124, 158, 255, 0.8); }

  /* Custom range slider */
  .field-slider {
    -webkit-appearance: none;
    appearance: none;
    width: 172px;
    height: 4px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.12);
    outline: none;
    cursor: pointer;
    flex-shrink: 0;
  }
  .field-slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #7c9eff;
    cursor: pointer;
    box-shadow: 0 1px 6px rgba(0, 0, 0, 0.5);
    transition: transform 0.1s;
  }
  .field-slider::-webkit-slider-thumb:hover { transform: scale(1.15); }

  /* ─── Hint ─── */
  .fixed-lang-check {
    display: flex;
    align-items: center;
    gap: 2px;
    cursor: pointer;
    font-size: 14px;
    flex-shrink: 0;
    color: #ffffff;
    opacity: 0.7;
    transition: opacity 0.15s;
  }
  .fixed-lang-check:hover { opacity: 1; }
  .fixed-lang-check input { width: 14px; height: 14px; accent-color: var(--primary); cursor: pointer; }
  .fixed-lang-check:has(input:checked) { opacity: 1; }

  .hint {
    margin: 8px 0 0;
    font-size: 10.5px;
    color: rgba(232, 232, 240, 0.3);
    font-style: italic;
    line-height: 1.5;
  }

  /* ─── Footer ─── */
  .footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 20px;
    border-top: 1px solid rgba(255, 255, 255, 0.06);
    flex-shrink: 0;
  }

  .btn {
    padding: 7px 20px;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.13);
    background: rgba(255, 255, 255, 0.06);
    color: rgba(232, 232, 240, 0.85);
    font-size: 13px;
    cursor: pointer;
    transition: background 0.1s;
  }
  .btn:hover { background: rgba(255, 255, 255, 0.11); }

  .btn.primary {
    background: rgba(124, 158, 255, 0.22);
    border-color: rgba(124, 158, 255, 0.42);
    color: #c8d8ff;
  }
  .btn.primary:hover { background: rgba(124, 158, 255, 0.35); }

  /* ── Capture button ── */
  .capture-btn {
    padding: 4px 12px;
    border-radius: 6px;
    border: 1px solid rgba(160, 168, 255, 0.32);
    background: rgba(160, 168, 255, 0.18);
    color: #A0A8FF;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: background 0.15s, border-color 0.15s;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
    min-width: 60px;
    text-align: center;
  }
  .capture-btn:hover:not(:disabled) {
    background: rgba(160, 168, 255, 0.32);
  }
  .capture-btn.active {
    border-color: rgba(100, 255, 200, 0.5);
    background: rgba(100, 255, 200, 0.12);
    color: rgba(100, 255, 200, 0.9);
    animation: capture-pulse 1s ease-in-out infinite;
  }
  .capture-btn.applying {
    border-color: rgba(255, 210, 80, 0.5);
    background: rgba(255, 210, 80, 0.12);
    color: rgba(255, 210, 80, 0.9);
    animation: capture-pulse 0.6s ease-in-out infinite;
  }
  .capture-btn:disabled {
    cursor: default;
  }
  @keyframes capture-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.5; }
  }

  /* ── Toggle switch ── */
  .toggle-wrap {
    display: flex;
    align-items: center;
  }
  .toggle-input { display: none; }
  .toggle-track {
    width: 36px;
    height: 20px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.1);
    border: 1px solid rgba(255, 255, 255, 0.15);
    position: relative;
    cursor: pointer;
    transition: background 0.2s, border-color 0.2s;
  }
  .toggle-input:checked + .toggle-track {
    background: rgba(100, 255, 200, 0.25);
    border-color: rgba(100, 255, 200, 0.5);
  }
  .toggle-thumb {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: rgba(232, 232, 240, 0.6);
    transition: transform 0.2s, background 0.2s;
  }
  .toggle-input:checked + .toggle-track .toggle-thumb {
    transform: translateX(16px);
    background: rgba(100, 255, 200, 0.9);
  }
  .save-error {
    color: rgba(255, 95, 86, 0.9);
    font-size: 11px;
    flex: 1;
    text-align: left;
    padding-left: 4px;
  }

  /* ─── Speaker-enrollment name dialog ─── */
  .enroll-modal-backdrop {
    position: absolute; inset: 0; z-index: 110;
    background: rgba(0, 0, 0, 0.55);
    display: flex; align-items: center; justify-content: center;
    -webkit-backdrop-filter: blur(4px); backdrop-filter: blur(4px);
  }
  .enroll-modal {
    min-width: 260px; max-width: 340px;
    background: rgba(14, 14, 26, 0.98);
    border: 1px solid rgba(160, 168, 255, 0.28);
    border-radius: 12px;
    padding: 14px 16px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.7);
  }
  .enroll-rename-link {
    display: inline-block;
    margin-top: 2px;
    padding: 0;
    background: none;
    border: none;
    font-size: 10.5px;
    color: rgba(160, 168, 255, 0.7);
    cursor: pointer;
    text-align: left;
  }
  .enroll-rename-link:hover {
    color: rgba(180, 188, 255, 0.95);
    text-decoration: underline;
  }
  .enroll-modal-title {
    margin: 0 0 10px 0;
    font-size: 12.5px;
    color: rgba(232, 234, 255, 0.92);
  }
  .enroll-modal-input {
    width: 100%;
    padding: 7px 10px;
    font-size: 13px;
    border-radius: 6px;
    border: 1px solid rgba(160, 168, 255, 0.32);
    background: rgba(255, 255, 255, 0.06);
    color: rgba(232, 234, 255, 0.95);
    outline: none;
    box-sizing: border-box;
  }
  .enroll-modal-input:focus {
    border-color: rgba(160, 168, 255, 0.6);
  }
  .enroll-modal-error {
    margin: 8px 0 0;
    font-size: 11px;
    color: rgba(255, 130, 130, 0.9);
  }
  .enroll-modal-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    margin-top: 12px;
  }
</style>
