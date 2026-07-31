<script lang="ts">
  let {
    label,
    value,
    lock,
    fixed,
    voices,
    lockText,
    valueChange,
    fixedChange,
    preview,
    previewText,
    previewingText,
    previewing = false,
    previewDisabled = false,
    optional = false,
    noneText = '',
  }: {
    label: string;
    value: string;
    lock: string;
    fixed: string;
    voices: readonly (readonly [string, string])[];
    lockText: string;
    valueChange: (value: string) => void;
    fixedChange: (value: string) => void;
    preview: () => void;
    previewText: string;
    previewingText: string;
    previewing?: boolean;
    previewDisabled?: boolean;
    optional?: boolean;
    noneText?: string;
  } = $props();
</script>

<div class="voice-field">
  <div class="field">
    <span class="field-label">{label}</span>
    <div class="voice-control">
      <select aria-label={label} {value} onchange={(event) => valueChange(event.currentTarget.value)}>
        {#if optional}<option value="">{noneText}</option>{/if}
        {#each voices as voice}<option value={voice[0]}>{voice[1]}</option>{/each}
      </select>
      <button
        class="preview-button"
        onclick={preview}
        disabled={!value || previewDisabled}
        aria-label={`${previewText}: ${label}`}
      >{previewing ? previewingText : previewText}</button>
    </div>
  </div>
  {#if value}
    <label class="check">
      <input
        type="checkbox"
        checked={fixed === lock}
        onchange={() => fixedChange(fixed === lock ? '' : lock)}
      />
      {lockText}
    </label>
  {/if}
</div>
