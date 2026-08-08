<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { SETTINGS_DEFAULTS, type AppSettings } from './lib/stores/settings';
  import Field from './lib/components/settings/Field.svelte';
  import Toggle from './lib/components/settings/Toggle.svelte';
  import RangeField from './lib/components/settings/RangeField.svelte';
  import VoiceField from './lib/components/settings/VoiceField.svelte';
  import AssetCard from './lib/components/settings/AssetCard.svelte';
  import {
    SETTINGS_LANGS,
    settingsText,
    type SettingsLang,
  } from './lib/settingsI18n';

  type Section = 'general' | 'avatar' | 'voice' | 'conversation' | 'interruption' | 'wake';
  type SaveResponse = { config: AppSettings; revision: number };
  type SettingsBootstrap = { config?: AppSettings; error?: string };

  declare global {
    interface Window {
      __PA_SETTINGS_BOOTSTRAP__?: SettingsBootstrap;
      __TAURI_INTERNALS__?: unknown;
    }
  }

  // Dynamic Tauri webviews do not consistently expose __TAURI_INTERNALS__ at
  // component initialization. The Rust-injected bootstrap is the reliable
  // marker for the real settings window.
  const isTauri = !!window.__PA_SETTINGS_BOOTSTRAP__ || !!window.__TAURI_INTERNALS__;
  const appWindow = isTauri ? getCurrentWindow() : null;
  const bootstrap = window.__PA_SETTINGS_BOOTSTRAP__
    ?? (!isTauri ? { config: SETTINGS_DEFAULTS } : undefined);
  const languageNames: Record<SettingsLang, string> = {
    zh: '中文', en: 'English', ja: '日本語', ko: '한국어',
    fr: 'Français', de: 'Deutsch', es: 'Español',
  };
  const voices = [
    ['zh-CN-XiaoxiaoNeural', '晓晓 · 中文'], ['zh-CN-XiaoyiNeural', '晓伊 · 中文'],
    ['zh-CN-YunxiNeural', '云希 · 中文'], ['zh-CN-YunjianNeural', '云健 · 中文'],
    ['zh-CN-YunyangNeural', '云扬 · 中文'], ['zh-CN-liaoning-XiaobeiNeural', '小北 · 东北话'],
    ['zh-CN-shaanxi-XiaoniNeural', '小妮 · 陕西话'], ['zh-HK-HiuGaaiNeural', '曉佳 · 粵語'],
    ['zh-HK-WanLungNeural', '雲龍 · 粵語'], ['zh-TW-HsiaoChenNeural', '曉臻 · 台灣'],
    ['zh-TW-YunJheNeural', '宥哲 · 台灣'], ['ja-JP-NanamiNeural', 'ななみ · 日本語'],
    ['ja-JP-KeitaNeural', 'けいた · 日本語'], ['en-US-AvaNeural', 'Ava · English US'],
    ['en-US-AndrewNeural', 'Andrew · English US'], ['en-US-JennyNeural', 'Jenny · English US'],
    ['en-GB-SoniaNeural', 'Sonia · English UK'], ['en-GB-LibbyNeural', 'Libby · English UK'],
    ['en-GB-MaisieNeural', 'Maisie · English UK'], ['en-GB-RyanNeural', 'Ryan · English UK'],
    ['en-GB-ThomasNeural', 'Thomas · English UK'],
    ['ko-KR-SunHiNeural', '선히 · 한국어'], ['ko-KR-InJoonNeural', '인준 · 한국어'],
    ['fr-FR-DeniseNeural', 'Denise · Français'], ['fr-FR-HenriNeural', 'Henri · Français'],
    ['de-DE-KatjaNeural', 'Katja · Deutsch'], ['de-DE-ConradNeural', 'Conrad · Deutsch'],
    ['es-ES-ElviraNeural', 'Elvira · Español'], ['es-ES-AlvaroNeural', 'Álvaro · Español'],
  ] as const;

  let active = $state<Section>('general');
  let local = $state<AppSettings | null>(bootstrap?.config ?? null);
  let baseline = $state(bootstrap?.config ? JSON.stringify(bootstrap.config) : '');
  let loading = $state(!bootstrap);
  let saving = $state(false);
  let notice = $state('');
  let error = $state(bootstrap?.error
    ? `${settingsText('en').loadError}: ${bootstrap.error}`
    : '');
  let avatarImageChanged = false;
  let avatarGifChanged = false;
  let capturing = $state(false);
  let captureTimer = $state<ReturnType<typeof setInterval> | null>(null);
  let enrollName = $state('Me');
  let showNameDialog = $state(false);
  let enrolling = $state(false);
  let enrollCountdown = $state(0);
  let variantCount = $state(0);
  let wakeWords = $state<string[]>([]);
  let pendingWakeDeletes = $state<string[]>([]);
  let previewingField = $state<'primary' | 'aux1' | 'aux2' | null>(null);
  let fileInput = $state<HTMLInputElement>(undefined!);
  let gifInput = $state<HTMLInputElement>(undefined!);

  let text = $derived(settingsText(local?.ui_lang ?? 'en'));
  let dirty = $derived((!!local && JSON.stringify(local) !== baseline) || pendingWakeDeletes.length > 0);
  let sections = $derived([
    { id: 'general' as Section, icon: '⌘', label: text.general },
    { id: 'avatar' as Section, icon: '◉', label: text.avatar },
    { id: 'voice' as Section, icon: '◖', label: text.voice },
    { id: 'conversation' as Section, icon: '◫', label: text.conversation },
    { id: 'interruption' as Section, icon: '↯', label: text.interruption },
    { id: 'wake' as Section, icon: '◎', label: text.wake },
  ]);

  async function loadSettings() {
    loading = true;
    error = '';
    try {
      const config = await Promise.race([
        invoke<AppSettings>('get_config'),
        new Promise<never>((_, reject) => {
          setTimeout(() => reject(new Error('get_config timed out after 8 seconds')), 8000);
        }),
      ]);
      local = config;
      baseline = JSON.stringify(config);
      await refreshVariantCount();
    } catch (e) {
      error = `${settingsText('en').loadError}: ${String(e)}`;
    } finally {
      loading = false;
    }
  }

  async function loadAssets() {
    if (!isTauri || !local) return;
    try {
      const [avatarImage, avatarGif] = await Promise.all([
        invoke<string | null>('get_setting_asset', { key: 'avatar_image' }),
        invoke<string | null>('get_setting_asset', { key: 'avatar_gif' }),
      ]);
      if (!local) return;
      const base = JSON.parse(baseline || JSON.stringify(local)) as AppSettings;
      if (!avatarImageChanged) {
        local = { ...local, avatar_image: avatarImage };
        base.avatar_image = avatarImage;
      }
      if (!avatarGifChanged) {
        local = { ...local, avatar_gif: avatarGif };
        base.avatar_gif = avatarGif;
      }
      baseline = JSON.stringify(base);
    } catch (e) {
      console.warn('[settings] avatar load failed:', e);
    }
  }

  if (bootstrap?.config) {
    void (async () => { await loadAssets(); await refreshVariantCount(); })();
  } else if (!bootstrap) {
    void loadSettings();
  }

  async function save() {
    if (!local || saving) return;
    saving = true; error = ''; notice = '';
    try {
      const avatarImage = local.avatar_image;
      const avatarGif = local.avatar_gif;
      const result = await invoke<SaveResponse>('save_settings_page_config', { config: local });
      if (avatarImageChanged) {
        if (avatarImage) await invoke('save_setting_asset', { key: 'avatar_image', dataUri: avatarImage });
        else await invoke('delete_setting_asset', { key: 'avatar_image' });
      }
      if (avatarGifChanged) {
        if (avatarGif) await invoke('save_setting_asset', { key: 'avatar_gif', dataUri: avatarGif });
        else await invoke('delete_setting_asset', { key: 'avatar_gif' });
      }
      local = { ...result.config, avatar_image: avatarImage, avatar_gif: avatarGif };
      avatarImageChanged = false;
      avatarGifChanged = false;
      if (pendingWakeDeletes.length && local.last_enrolled_speaker) {
        for (const w of pendingWakeDeletes) {
          wakeWords = await invoke<string[]>('remove_wake_word', { name: local.last_enrolled_speaker, word: w });
        }
        variantCount = wakeWords.length;
        pendingWakeDeletes = [];
      }
      baseline = JSON.stringify(local);
      notice = text.saved;
    } catch (e) {
      error = `${text.saveError}: ${String(e)}`;
    } finally {
      saving = false;
    }
  }

  async function cancel() {
    if (dirty && !window.confirm(text.dirtyConfirm)) return;
    pendingWakeDeletes = [];
    baseline = local ? JSON.stringify(local) : '';
    if (!appWindow) {
      error = `${text.actionError}: settings window handle is unavailable`;
      return;
    }
    try {
      await appWindow.close();
    } catch (e) {
      error = `${text.actionError}: ${String(e)}`;
    }
  }

  async function persistImmediate(partial: Partial<AppSettings>) {
    if (!local) throw new Error(text.loadError);
    const result = await invoke<SaveResponse>('save_settings_page_config', {
      config: { ...local, ...partial },
    });
    if (local) local = { ...local, ...partial };
    const base = JSON.parse(baseline || JSON.stringify(local)) as AppSettings;
    baseline = JSON.stringify({ ...base, ...partial });
    return result.config;
  }

  async function captureHotkey() {
    if (capturing) return;
    error = ''; notice = ''; capturing = true;
    try {
      await invoke('start_capture');
    } catch (e) {
      capturing = false; error = `${text.actionError}: ${String(e)}`; return;
    }
    let attempts = 0;
    captureTimer = setInterval(async () => {
      attempts++;
      try {
        const result = await invoke<[number, string] | null>('poll_capture');
        if (result) {
          if (captureTimer) clearInterval(captureTimer);
          captureTimer = null;
          await persistImmediate({ hotkey_code: result[0], hotkey_name: result[1] });
          capturing = false;
          notice = text.saved;
        } else if (attempts > 75) {
          if (captureTimer) clearInterval(captureTimer);
          captureTimer = null; capturing = false;
        }
      } catch (e) {
        if (captureTimer) clearInterval(captureTimer);
        captureTimer = null; capturing = false; error = `${text.actionError}: ${String(e)}`;
      }
    }, 80);
  }

  async function previewVoice(voice: string, field: 'primary' | 'aux1' | 'aux2') {
    if (!local || !voice || previewingField) return;
    previewingField = field;
    error = '';
    notice = '';
    try {
      await invoke('preview_voice', {
        voice,
        ttsFormat: local.tts_format,
        volume: local.volume,
      });
    } catch (e) {
      error = `${text.actionError}: ${String(e)}`;
    } finally {
      previewingField = null;
    }
  }

  function chooseAsset(kind: 'avatar_image' | 'avatar_gif') {
    (kind === 'avatar_image' ? fileInput : gifInput)?.click();
  }

  function readAsset(event: Event, kind: 'avatar_image' | 'avatar_gif') {
    const file = (event.currentTarget as HTMLInputElement).files?.[0];
    if (!file || !local) return;
    if (file.size > 10 * 1024 * 1024 || !['image/jpeg','image/png','image/webp','image/gif'].includes(file.type)) {
      error = `${text.actionError}: ${text.imageHint}`; return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      if (!local) return;
      local = { ...local, [kind]: String(reader.result) };
      if (kind === 'avatar_image') avatarImageChanged = true;
      else avatarGifChanged = true;
    };
    reader.onerror = () => error = text.actionError;
    reader.readAsDataURL(file);
  }

  async function refreshVariantCount() {
    if (!local?.last_enrolled_speaker) { variantCount = 0; wakeWords = []; return; }
    try {
      variantCount = await invoke<number>('get_wake_variant_count', { name: local.last_enrolled_speaker });
      wakeWords = await invoke<string[]>('get_wake_words', { name: local.last_enrolled_speaker });
    } catch { variantCount = 0; wakeWords = []; }
  }

  function toggleWakeDelete(word: string) {
    pendingWakeDeletes = pendingWakeDeletes.includes(word)
      ? pendingWakeDeletes.filter((w) => w !== word)
      : [...pendingWakeDeletes, word];
  }

  function requestEnrollment(rename = false) {
    if (!local) return;
    enrollName = rename ? (local.last_enrolled_speaker || 'Me') : (local.last_enrolled_speaker || 'Me');
    showNameDialog = true;
  }

  async function beginEnrollment(training = false) {
    if (!/^[A-Za-z0-9_-]{1,32}$/.test(enrollName)) {
      error = text.validationName; return;
    }
    showNameDialog = false; enrolling = true; enrollCountdown = 3; error = ''; notice = '';
    try {
      await invoke('stop_wake_word_listening').catch(() => {});
      await invoke('start_enroll_recording');
      const timer = setInterval(() => enrollCountdown--, 1000);
      await new Promise(resolve => setTimeout(resolve, 3000));
      clearInterval(timer);
      const wavPath = await invoke<string>('stop_enroll_recording');
      if (training) {
        await invoke('train_speaker', { name: enrollName, audioPath: wavPath });
      } else {
        await invoke('enroll_speaker', { name: enrollName, audioPath: wavPath });
      }
      await persistImmediate({ last_enrolled_speaker: enrollName });
      await refreshVariantCount();
      notice = training ? text.trainSuccess : text.enrollSuccess;
    } catch (e) {
      error = `${text.actionError}: ${String(e)}`;
    } finally {
      enrolling = false; enrollCountdown = 0;
      invoke<AppSettings>('get_config').then((current) => {
        if (current.wake_word_enabled) {
          return invoke('start_wake_word_listening', {
            threshold: current.wake_word_threshold,
            speakerName: current.last_enrolled_speaker || 'Me',
          });
        }
      }).catch(() => {});
    }
  }

  async function train() {
    if (!local?.last_enrolled_speaker) return requestEnrollment();
    enrollName = local.last_enrolled_speaker;
    await beginEnrollment(true);
  }
</script>

<svelte:head><title>Pocket Agent Settings</title></svelte:head>

<div class="shell">
  <aside>
    <div class="brand">
      <div class="logo">PA</div>
      <div><h1>{text.title}</h1><p>{text.subtitle}</p></div>
    </div>
    <nav aria-label={text.navAria}>
      {#each sections as section}
        <button class:active={active === section.id} onclick={() => active = section.id}>
          <span>{section.icon}</span>{section.label}
        </button>
      {/each}
    </nav>
    <p class="immediate">{text.immediateHint}</p>
  </aside>

  <main aria-label={text.contentAria}>
    {#if loading}
      <div class="center"><span class="spinner"></span></div>
    {:else if local}
      <header><div><h2>{sections.find(s => s.id === active)?.label}</h2><p>{text.subtitle}</p></div></header>
      <div class="content">
        {#if active === 'general'}
          <section class="card">
            <Field label={text.uiLanguage}>
              <select bind:value={local.ui_lang}>
                {#each SETTINGS_LANGS as lang}<option value={lang}>{languageNames[lang]}</option>{/each}
              </select>
            </Field>
            <Field label={text.recordKey}>
              <button class="secondary" class:working={capturing} onclick={captureHotkey} disabled={capturing}>
                {capturing ? text.capturing : `${text.capture}: ${local.hotkey_name}`}
              </button>
            </Field>
            <Toggle label={text.doubleClick} value={local.double_click_to_record} change={() => local && (local.double_click_to_record = !local.double_click_to_record)} />
          </section>
        {:else if active === 'avatar'}
          <section class="card asset-grid">
            <AssetCard title={text.staticAvatar} src={local.avatar_image} hint={text.imageHint}
              choose={() => chooseAsset('avatar_image')} remove={() => {
                if (local) local.avatar_image = null;
                avatarImageChanged = true;
              }}
              chooseText={text.chooseImage} removeText={text.remove} />
            <AssetCard title={text.animatedAvatar} src={local.avatar_gif} hint={text.imageHint}
              choose={() => chooseAsset('avatar_gif')} remove={() => {
                if (local) local.avatar_gif = null;
                avatarGifChanged = true;
              }}
              chooseText={text.chooseImage} removeText={text.remove} />
            <input bind:this={fileInput} hidden type="file" accept="image/jpeg,image/png,image/webp,image/gif" onchange={(e) => readAsset(e, 'avatar_image')} />
            <input bind:this={gifInput} hidden type="file" accept="image/gif" onchange={(e) => readAsset(e, 'avatar_gif')} />
          </section>
        {:else if active === 'voice'}
          <section class="card">
            <Toggle label={text.voiceOutput} value={local.tts_enabled} change={() => local && (local.tts_enabled = !local.tts_enabled)} />
            <VoiceField label={text.primaryVoice} value={local.tts_primary_voice} lock="primary" fixed={local.fixed_lang} {voices} lockText={text.lockLanguage}
              preview={() => previewVoice(local!.tts_primary_voice, 'primary')} previewText={text.previewVoice} previewingText={text.previewingVoice}
              previewing={previewingField === 'primary'} previewDisabled={previewingField !== null}
              valueChange={(v) => local && (local.tts_primary_voice = v)} fixedChange={(v) => local && (local.fixed_lang = v)} />
            <VoiceField label={text.auxiliary1} value={local.tts_aux1_voice} lock="aux1" fixed={local.fixed_lang} {voices} lockText={text.lockLanguage} optional noneText={text.none}
              preview={() => previewVoice(local!.tts_aux1_voice, 'aux1')} previewText={text.previewVoice} previewingText={text.previewingVoice}
              previewing={previewingField === 'aux1'} previewDisabled={previewingField !== null}
              valueChange={(v) => local && (local.tts_aux1_voice = v)} fixedChange={(v) => local && (local.fixed_lang = v)} />
            <VoiceField label={text.auxiliary2} value={local.tts_aux2_voice} lock="aux2" fixed={local.fixed_lang} {voices} lockText={text.lockLanguage} optional noneText={text.none}
              preview={() => previewVoice(local!.tts_aux2_voice, 'aux2')} previewText={text.previewVoice} previewingText={text.previewingVoice}
              previewing={previewingField === 'aux2'} previewDisabled={previewingField !== null}
              valueChange={(v) => local && (local.tts_aux2_voice = v)} fixedChange={(v) => local && (local.fixed_lang = v)} />
            <Field label={text.audioFormat}><select bind:value={local.tts_format}><option value="wav">WAV</option><option value="mp3">MP3</option></select></Field>
            <RangeField label={text.volume} value={local.volume} min={0} max={1} step={0.05} display={`${Math.round(local.volume * 100)}${text.percent}`} change={(v) => local && (local.volume = v)} />
          </section>
        {:else if active === 'conversation'}
          <section class="card">
            <Toggle label={text.continuous} hint={text.hintContinuous} value={local.continuous_conversation} change={() => local && (local.continuous_conversation = !local.continuous_conversation)} />
            <RangeField label={text.silenceTimeout} hint={text.hintSilenceTimeout} value={local.silence_timeout_secs} min={3} max={10} step={1} display={`${local.silence_timeout_secs} ${text.seconds}`} change={(v) => local && (local.silence_timeout_secs = v)} />
            <RangeField label={text.pauseTolerance} hint={text.hintPauseTolerance} value={local.pause_tolerance_ms} min={500} max={5000} step={100} display={`${local.pause_tolerance_ms} ${text.milliseconds}`} change={(v) => local && (local.pause_tolerance_ms = v)} />
            <RangeField label={text.micSensitivity} hint={text.hintMicSensitivity} value={local.speech_rms_threshold} min={0.003} max={0.020} step={0.001} display={local.speech_rms_threshold.toFixed(3)} change={(v) => local && (local.speech_rms_threshold = v)} />
          </section>
        {:else if active === 'interruption'}
          <section class="card">
            <Toggle label={text.allowInterruption} hint={text.hintAllowInterruption} value={local.barge_in_enabled} change={() => local && (local.barge_in_enabled = !local.barge_in_enabled)} />
            <RangeField label={text.interruptSensitivity} hint={text.hintInterruptSensitivity} value={local.barge_in_rms_threshold} min={0.02} max={0.15} step={0.01} display={local.barge_in_rms_threshold.toFixed(2)} change={(v) => local && (local.barge_in_rms_threshold = v)} />
            <Toggle label={text.skipConfirmation} hint={text.hintSkipConfirmation} value={local.skip_interrupt_confirmation} change={() => local && (local.skip_interrupt_confirmation = !local.skip_interrupt_confirmation)} />
          </section>
        {:else if active === 'wake'}
          <section class="card">
            <Toggle label={text.wakeEnabled} hint={text.hintWakeEnabled} value={local.wake_word_enabled} change={() => local && (local.wake_word_enabled = !local.wake_word_enabled)} />
            <RangeField label={text.wakeSensitivity} hint={text.hintWakeSensitivity} value={local.wake_word_threshold} min={0.30} max={0.90} step={0.05} display={`${Math.round(local.wake_word_threshold * 100)}${text.percent}`} change={(v) => local && (local.wake_word_threshold = v)} />
            <div class="sample">
              <div><strong>{text.sample}</strong><p>{local.last_enrolled_speaker || text.noSample}{variantCount ? ` · ${variantCount} ${text.variants}` : ''}</p></div>
              <div class="actions">
                {#if local.last_enrolled_speaker}<button class="ghost" onclick={() => requestEnrollment(true)}>{text.rename}</button>{/if}
                <button class="secondary" onclick={() => requestEnrollment()} disabled={enrolling}>{enrolling ? `${text.recording} ${enrollCountdown}` : text.recordSample}</button>
                {#if local.last_enrolled_speaker}<button class="secondary" onclick={train} disabled={enrolling}>{text.trainSample}</button>{/if}
              </div>
            </div>
            {#if local.last_enrolled_speaker}
              <div class="wake-words">
                <span class="wake-label">{text.wakeWordsLabel}</span>
                {#if wakeWords.length > 0}
                  <div class="wake-tags">
                    {#each wakeWords as word}
                      <span class="wake-tag" class:pending-delete={pendingWakeDeletes.includes(word)}>
                        {word}
                        <button class="wake-del" aria-label={text.remove} onclick={() => toggleWakeDelete(word)}>×</button>
                      </span>
                    {/each}
                  </div>
                {:else}
                  <p class="wake-empty">{text.wakeWordsEmpty}</p>
                {/if}
              </div>
            {/if}
          </section>
        {/if}
      </div>
      <footer>
        <div class="message"><span class:error>{error || notice}</span></div>
        <button class="ghost" onclick={cancel}>{text.cancel}</button>
        <button class="primary" onclick={save} disabled={!dirty || saving}>{saving ? text.saving : text.save}</button>
      </footer>
    {:else}
      <div class="center error-state">
        <p class="error">{error || text.loadError}</p>
        <button class="secondary" onclick={loadSettings}>{text.retry}</button>
      </div>
    {/if}
  </main>
</div>

{#if showNameDialog}
  <div class="backdrop">
    <div class="modal">
      <h3>{text.sample}</h3><p>{text.enrollHint}</p>
      <label>{text.speakerName}<input bind:value={enrollName} maxlength="32" /></label>
      <div class="modal-actions"><button class="ghost" onclick={() => showNameDialog = false}>{text.cancel}</button><button class="primary" onclick={() => beginEnrollment(false)}>{text.startRecording}</button></div>
    </div>
  </div>
{/if}

<style>
  :global {
  :global(*){box-sizing:border-box} :global(html,body,#app){margin:0;width:100%;height:100%;overflow:hidden}
  :global(body){font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;background:#f4f5f9;color:#1e2235;-webkit-font-smoothing:antialiased}
  button,select,input{font:inherit}.shell{height:100%;min-height:0;display:grid;grid-template-columns:200px 1fr;background:radial-gradient(circle at 75% 0,#dde3ff 0,transparent 36%),#f4f5f9}
  aside{padding:20px 12px 16px;border-right:1px solid #e2e5ed;background:#ffffff;display:flex;flex-direction:column}.brand{display:flex;gap:10px;align-items:center;padding:0 8px 18px}
  .logo{width:34px;height:34px;border-radius:10px;display:grid;place-items:center;background:linear-gradient(145deg,#859bff,#7465e7);font-size:12px;font-weight:800;box-shadow:0 6px 18px #6174ff35;color:#fff}
  h1,h2,h3,p{margin:0}.brand h1{font-size:14px;color:#1e2235}.brand p,header p{font-size:10px;color:#6a7185;margin-top:3px}nav{display:flex;flex-direction:column;gap:3px}nav button{border:0;background:transparent;color:#5a6178;padding:8px 10px;border-radius:8px;text-align:left;cursor:pointer;display:flex;gap:10px;align-items:center;font-size:12px}nav button span{width:18px;text-align:center;color:#8890a8;font-size:13px}nav button:hover{background:#0000000a;color:#2a2f42}nav button.active{background:#788cff18;color:#3a4566;box-shadow:inset 0 0 0 1px #8294ff30}nav button.active span{color:#7080ff}.immediate{margin-top:auto;padding:10px;font-size:9px;line-height:1.5;color:#939aab}
  main{min-width:0;min-height:0;height:100%;display:grid;grid-template-rows:58px minmax(0,1fr) 52px;overflow:hidden}header{padding:14px 28px;border-bottom:1px solid #e2e5ed;display:flex;align-items:center}header h2{font-size:17px;color:#1e2235}.content{min-height:0;padding:18px 28px;overflow:auto;overscroll-behavior:contain}.card{max-width:680px;border:1px solid #e2e5ed;border-radius:12px;background:#ffffff;overflow:hidden;box-shadow:0 3px 14px #0000000a}
  .field{min-height:46px;padding:9px 16px;display:flex;align-items:center;justify-content:space-between;gap:20px;border-bottom:1px solid #e8eaf0}.field:last-child{border-bottom:0}.field-label{font-size:12px;color:#2a2f42}.field small{display:block;color:#7a8298;margin-top:3px;font-size:10px;font-weight:500}.field-hint{display:block;color:#9aa3b8;margin-top:3px;font-size:10px;font-style:normal;line-height:1.4}.control{min-width:220px;display:flex;justify-content:flex-end}
  select,.modal input{width:220px;border:1px solid #d4d8e0;background:#ffffff;color:#1e2235;border-radius:7px;padding:6px 10px;outline:none;font-size:12px}select:focus,input:focus{border-color:#7589f4}input[type=range]{width:220px;accent-color:#7589f4}.toggle{width:38px;height:22px;padding:2px;border:0;border-radius:18px;background:#d4d8e0;cursor:pointer;transition:.15s}.toggle span{display:block;width:18px;height:18px;border-radius:50%;background:#ffffff;transition:.15s;box-shadow:0 1px 3px #00000030}.toggle.on{background:#7589f4}.toggle.on span{transform:translateX(16px);background:white}
  button.primary,button.secondary,button.ghost{border-radius:7px;padding:6px 12px;cursor:pointer;border:1px solid transparent;color:#1e2235;font-size:12px}button.primary{background:#7589f4;color:#fff}button.primary:hover{background:#6478e8}button.primary:disabled{opacity:.4;cursor:default}button.secondary{background:#f0f2f8;border-color:#d4d8e0;color:#1e2235}button.secondary:hover{background:#e8eaf2}button.ghost{background:transparent;border-color:#d4d8e0;color:#5a6178}button.ghost:hover{background:#0000000a}.danger{color:#e53e5c!important}.working{color:#c99700!important}
  footer{min-height:52px;border-top:1px solid #e2e5ed;padding:10px 28px;display:flex;gap:8px;align-items:center;justify-content:flex-end;background:#ffffff;position:relative;z-index:2}.message{margin-right:auto;color:#1a9d63;font-size:11px}.message .error{color:#e53e5c}
  .voice-field{border-bottom:1px solid #e8eaf0}.voice-field .field{border:0;padding-bottom:5px}.voice-control{display:flex;align-items:center;gap:6px}.voice-control select{width:165px}.preview-button{width:84px;min-width:84px;max-width:84px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;border:1px solid #d4d8e0;background:#f0f2f8;color:#1e2235;border-radius:7px;padding:6px 6px;cursor:pointer;font-size:11px}.preview-button:hover{background:#e8eaf2}.preview-button:disabled{opacity:.45;cursor:default}.check{display:flex;justify-content:flex-end;align-items:center;gap:6px;padding:0 16px 9px;color:#7a8298;font-size:10px}.check input{accent-color:#7589f4}.asset-grid{display:grid;grid-template-columns:1fr 1fr;gap:0}.asset{padding:18px;text-align:center}.asset+ .asset{border-left:1px solid #e2e5ed}.preview{width:84px;height:84px;margin:0 auto 12px;border-radius:18px;background:linear-gradient(145deg,#f0f2f8,#e8eaf2);border:1px solid #d4d8e0;display:grid;place-items:center;overflow:hidden;color:#7589f4;font-weight:800}.preview img{width:100%;height:100%;object-fit:cover}.asset h3{font-size:13px;color:#1e2235}.asset p{height:32px;margin:5px 0 10px;color:#7a8298;font-size:10px;line-height:1.5}.asset button+button{margin-left:6px}
  .wake-words{padding:12px 16px;border-top:1px solid #e8eaf0}.wake-label{display:block;font-size:12px;font-weight:600;color:#1e2235;margin-bottom:8px}.wake-tags{display:flex;flex-wrap:wrap;gap:6px}.wake-tag{display:inline-flex;align-items:center;gap:4px;background:#eef1f7;border:1px solid #dde2ec;color:#3a4156;font-size:11px;font-weight:500;padding:4px 6px 4px 10px;border-radius:14px}
.wake-tag.pending-delete{opacity:.5;text-decoration:line-through}
.wake-del{display:inline-flex;align-items:center;justify-content:center;width:16px;height:16px;padding:0;border:none;background:transparent;color:#8a91a6;font-size:14px;line-height:1;border-radius:8px;cursor:pointer;text-decoration:none}
.wake-del:hover{background:#dce0ea;color:#c0392b}
.wake-empty{margin:0;font-size:11px;color:#8a91a6}.sample{padding:16px;display:flex;align-items:center;justify-content:space-between;gap:16px}.sample strong{font-size:12px;color:#1e2235}.sample p{font-size:10px;color:#7a8298;margin-top:4px}.actions{display:flex;gap:6px;flex-wrap:wrap;justify-content:flex-end}.center{height:100%;display:grid;place-items:center;color:#8e97aa}.spinner{width:22px;height:22px;border:2px solid #d4d8e0;border-top-color:#7589f4;border-radius:50%;animation:spin .8s linear infinite}@keyframes spin{to{transform:rotate(360deg)}}.error{color:#e53e5c}
  .error-state{align-content:center;justify-items:center;gap:12px;padding:24px;text-align:center}.error-state p{max-width:480px;line-height:1.5}
  .backdrop{position:fixed;inset:0;background:#00000033;display:grid;place-items:center;z-index:20}.modal{width:360px;background:#ffffff;border:1px solid #e2e5ed;border-radius:12px;padding:20px;box-shadow:0 20px 60px #00000030}.modal h3{font-size:15px;color:#1e2235}.modal p{color:#6a7185;font-size:11px;line-height:1.5;margin:6px 0 16px}.modal label{display:grid;gap:6px;font-size:11px;color:#2a2f42}.modal input{width:100%}.modal-actions{display:flex;justify-content:flex-end;gap:8px;margin-top:18px}
  @media(max-width:740px){.shell{grid-template-columns:170px 1fr}.content,header,footer{padding-left:18px;padding-right:18px}.field{gap:12px}.control,select,input[type=range]{min-width:0;width:190px}.asset-grid{grid-template-columns:1fr}.asset+.asset{border-left:0;border-top:1px solid #e2e5ed}}
  }
</style>
