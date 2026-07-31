<script lang="ts">
  import { chatStore } from '../stores/chat';
  import { characterState } from '../stores/character';
  import { settingsStore } from '../stores/settings';
  import { convLabelsLang } from '../i18n';

  let labels = $derived(convLabelsLang($settingsStore.ui_lang));

  let showSetup = $derived(
    $chatStore.voiceSetupState === 'installing' ||
    $chatStore.voiceSetupState === 'error'
  );

  let hasContent = $derived(
    showSetup ||
    $chatStore.thinkingSteps.length > 0 ||
    $characterState === 'thinking' ||
    $chatStore.voiceStatus !== null
  );
</script>

  <div class="status-panel" class:visible={hasContent}>
    <div class="status-content">
      <div class="steps">
        {#if showSetup}
          {#if $chatStore.voiceSetupState === 'error'}
            <span class="step error">{labels.voiceSetupError}</span>
            {#if $chatStore.voiceSetupDetail}
              <span class="step error-detail">{$chatStore.voiceSetupDetail}</span>
            {/if}
          {:else}
            <span class="step">{labels.voiceSetupInstalling}</span>
            {#if $chatStore.voiceSetupPhase}
              <span class="step placeholder">↳ {labels.voiceSetupPhase($chatStore.voiceSetupPhase)}</span>
            {/if}
          {/if}
        {/if}
        {#if $chatStore.voiceStatus !== null}
          <span class="step">{$chatStore.voiceStatus}</span>
        {/if}
        {#if $chatStore.thinkingSteps.length > 0}
          {#each $chatStore.thinkingSteps as step}
            <span class="step">{step}</span>
          {/each}
        {:else if $characterState === 'thinking'}
          <span class="step placeholder">🤔 正在思考...</span>
        {/if}
      </div>
    </div>
  </div>

<style>
  .status-panel {
    width: 100%;
    background: rgba(10, 10, 22, 0.72);
    -webkit-backdrop-filter: blur(16px) saturate(140%);
    backdrop-filter: blur(16px) saturate(140%);
    border: 1px solid rgba(160, 168, 255, 0.18);
    border-radius: 8px;
    overflow: hidden;
    /* Always take up space to prevent layout shift */
    opacity: 0;
    border-color: transparent;
    background: transparent;
    -webkit-backdrop-filter: none;
    backdrop-filter: none;
    transition: opacity 0.2s ease;
  }
  .status-panel.visible {
    opacity: 1;
    border-color: rgba(160, 168, 255, 0.18);
    background: rgba(10, 10, 22, 0.72);
    -webkit-backdrop-filter: blur(16px) saturate(140%);
    backdrop-filter: blur(16px) saturate(140%);
  }

  .status-content {
    padding: 6px 10px;
  }

  .steps {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .step {
    font-size: 10.5px;
    line-height: 1.45;
    color: rgba(160, 168, 255, 0.65);
    word-break: break-word;
    animation: fade-in 0.25s ease-out;
  }

  .step.placeholder {
    color: rgba(160, 168, 255, 0.45);
  }

  .step.error {
    color: rgba(255, 140, 140, 0.85);
  }

  .step.error-detail {
    color: rgba(255, 140, 140, 0.55);
    font-size: 9.5px;
  }

  @keyframes fade-in {
    from { opacity: 0; transform: translateY(-3px); }
    to   { opacity: 1; transform: translateY(0); }
  }
</style>
