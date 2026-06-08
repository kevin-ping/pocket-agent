<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { settingsStore } from '../stores/settings';
  import { convLabelsLang } from '../i18n';

  export let mode: 'idle' | 'recording' | 'thinking' | 'waiting_for_wake' | 'verifying_speaker' = 'idle';
  export let audioLevel: number = 0;

  const dispatch = createEventDispatcher<{ click: void }>();

  $: tooltip = mode === 'waiting_for_wake'
    ? convLabelsLang($settingsStore.ui_lang).waitingForWakeCaption
    : mode === 'verifying_speaker'
      ? convLabelsLang($settingsStore.ui_lang).verifyingSpeakerCaption
      : '';
</script>

<!-- svelte-ignore a11y-click-events-have-key-events -->
<!-- svelte-ignore a11y-no-static-element-interactions -->
<div
  class="island-wrap"
  class:waiting-for-wake={mode === 'waiting_for_wake'}
  class:verifying-speaker={mode === 'verifying_speaker'}
  title={tooltip}
  on:click={() => dispatch('click')}
>
  <!-- Dot 1: collapses to dot, expands to voice wave -->
  <div class="island-bar bar-1" class:expanded={mode === 'recording'}>
    {#if mode === 'recording'}
      <div class="voice-wave">
        {#each Array(8) as _, i}
          <div class="wave-bar" style="--i: {i}; --level: {audioLevel}"></div>
        {/each}
      </div>
    {:else}
      <div class="dot"></div>
    {/if}
  </div>

  <!-- Dot 2: collapses to dot, expands to ECG line -->
  <div class="island-bar bar-2" class:expanded={mode === 'thinking'}>
    {#if mode === 'thinking'}
      <svg class="ecg-line" viewBox="0 0 72 16" preserveAspectRatio="none">
        <polyline class="ecg-trace"
          points="0,8 10,8 14,8 17,3 20,13 23,5 26,8 30,8 40,8 44,8 47,3 50,13 53,5 56,8 60,8 72,8"
          fill="none"
          stroke="rgba(100, 200, 255, 0.9)"
          stroke-width="1.5"
          stroke-linejoin="round"
          stroke-linecap="round"
        />
      </svg>
    {:else}
      <div class="dot"></div>
    {/if}
  </div>

  <!-- Dot 3: always a dot, wrapped in bar for consistent alignment -->
  <div class="island-bar bar-3">
    <div class="dot"></div>
  </div>
</div>

<style>
  .island-wrap {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 5px;
    margin-top: 0;
    cursor: pointer;
    user-select: none;
  }

  .island-bar {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 16px;
    width: 16px;
    border-radius: 999px;
    background: transparent;
    border: 2px solid var(--ring, rgba(120, 140, 255, 0.25));
    transition: width 0.4s cubic-bezier(0.4, 0, 0.2, 1),
                background 0.3s ease,
                border-color 0.3s ease;
    overflow: hidden;
    box-sizing: border-box;
  }

  .island-bar.expanded {
    width: 68px;
    background: rgba(14, 14, 26, 0.8);
  }

  .bar-1.expanded {
    border-color: rgba(100, 200, 255, 0.4);
  }

  .bar-2.expanded {
    border-color: rgba(180, 120, 255, 0.4);
  }

  /* ── Dot ── */
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--ring, rgba(160, 170, 255, 0.6));
    flex-shrink: 0;
  }

  /* ── Voice wave (dot 1) ── */
  .voice-wave {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 2px;
    width: 100%;
    height: 100%;
    padding: 2px 0;
  }

  .wave-bar {
    width: 2px;
    min-height: 2px;
    max-height: 10px;
    border-radius: 2px;
    background: linear-gradient(180deg, rgba(100, 200, 255, 0.9), rgba(160, 170, 255, 0.6));
    /* Dynamic level from audio — fallback wave animation when silent */
    height: calc(3px + var(--level, 0) * 7px);
    animation: wave-bounce 0.6s ease-in-out infinite alternate;
    animation-delay: calc(var(--i) * 0.05s);
  }

  @keyframes wave-bounce {
    0%   { height: 2px; }
    100% { height: 8px; }
  }

  /* ── ECG / heartbeat (dot 2) ── */
  .ecg-line {
    width: 80%;
    height: 14px;
    flex-shrink: 0;
  }

  .ecg-trace {
    stroke-dasharray: 120;
    stroke-dashoffset: 120;
    animation: ecg-draw 1.8s linear infinite;
  }

  @keyframes ecg-draw {
    0%   { stroke-dashoffset: 120; }
    100% { stroke-dashoffset: 0; }
  }

  /* ── Wake-listening state: gentle teal pulse on dot 3 ── */
  .island-wrap.waiting-for-wake .bar-3 {
    border-color: rgba(100, 220, 200, 0.55);
    animation: wake-pulse 1.8s ease-in-out infinite;
  }
  .island-wrap.waiting-for-wake .bar-3 .dot {
    background: rgba(100, 220, 200, 0.85);
  }
  @keyframes wake-pulse {
    0%, 100% { opacity: 0.55; }
    50%      { opacity: 1; }
  }

  /* ── Verifying-speaker state: amber pulse across all 3 dots ── */
  .island-wrap.verifying-speaker .island-bar {
    border-color: rgba(255, 200, 100, 0.55);
    animation: verify-pulse 0.7s ease-in-out infinite;
  }
  .island-wrap.verifying-speaker .island-bar .dot {
    background: rgba(255, 200, 100, 0.85);
  }
  @keyframes verify-pulse {
    0%, 100% { opacity: 0.6; }
    50%      { opacity: 1; }
  }
</style>
