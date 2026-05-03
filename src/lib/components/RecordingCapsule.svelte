<script lang="ts">
  import { fly } from 'svelte/transition';
  import { characterState } from '../stores/character';

  let visible = $derived($characterState === "listening" || $characterState === "thinking");
  let isListening = $derived($characterState === "listening");

  let seconds = $state(0);
  let timer: ReturnType<typeof setInterval> | null = null;

  $effect(() => {
    if (isListening) {
      seconds = 0;
      timer = setInterval(() => seconds++, 1000);
      return () => {
        if (timer !== null) clearInterval(timer);
      };
    }
  });
</script>

{#if visible}
  <div
    class="capsule"
    class:listening={isListening}
    in:fly={{ y: 16, duration: 180 }}
    out:fly={{ y: 16, duration: 130 }}
    aria-live="polite"
    aria-label={isListening ? `正在录音 ${seconds} 秒` : '正在处理'}
  >
    {#if isListening}
      <!-- CSS 波形：5根柱，只用 transform: scaleY，旧 GPU 安全 -->
      <div class="waveform" aria-hidden="true">
        {#each [0, 0.1, 0.2, 0.1, 0.05] as delay}
          <div class="bar" style="animation-delay: {delay}s"></div>
        {/each}
      </div>
      <span class="label">正在聆听</span>
      <span class="timer">{seconds}s</span>
    {:else}
      <span class="dot thinking-dot" aria-hidden="true"></span>
      <span class="label">正在处理...</span>
    {/if}
  </div>
{/if}

<style>
  .capsule {
    display: inline-flex;
    /* rgba 替代 backdrop-filter，兼容 macOS 10.13 */
    background: rgba(18, 18, 32, 0.94);
    border: 1px solid rgba(160, 168, 255, 0.35);
    border-radius: 20px;
    padding: 6px 14px;
    display: flex;
    align-items: center;
    gap: 7px;
    white-space: nowrap;
    z-index: 10;
  }

  .label {
    font-size: 12px;
    color: rgba(232, 232, 240, 0.88);
  }

  .timer {
    font-size: 11px;
    color: rgba(160, 168, 255, 0.75);
    min-width: 22px;
  }

  /* ── CSS 波形 ── */
  .waveform {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 14px;
  }

  @keyframes wave-bar {
    0%, 100% { transform: scaleY(0.25); }
    50%       { transform: scaleY(1);    }
  }

  .bar {
    width: 3px;
    height: 100%;
    background: #A0A8FF;
    border-radius: 2px;
    transform-origin: center;
    transform: scaleY(0.25);
    animation: wave-bar 0.55s ease-in-out infinite;
  }

  /* ── thinking 点 ── */
  @keyframes pulse-dot {
    0%, 100% { opacity: 0.3; transform: scale(0.8); }
    50%       { opacity: 1;   transform: scale(1);   }
  }

  .thinking-dot {
    display: inline-block;
    width: 7px;
    height: 7px;
    background: #A0A8FF;
    border-radius: 50%;
    animation: pulse-dot 1s ease-in-out infinite;
  }
</style>
