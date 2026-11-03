<script lang="ts">
  let {
    label,
    value,
    min,
    max,
    step,
    display,
    hint = '',
    change,
  }: {
    label: string;
    value: number;
    min: number;
    max: number;
    step: number;
    display: string;
    hint?: string;
    change: (value: number) => void;
  } = $props();

  // A range input normalizes its native value against the bounds available at
  // creation time. Re-apply the database value after min/max/step are mounted.
  let rangeInput: HTMLInputElement;
  $effect(() => {
    if (rangeInput) rangeInput.value = String(value);
  });
</script>

<div class="field range">
  <span class="field-label">{label}<small>{display}</small>{#if hint}<em class="field-hint">{hint}</em>{/if}</span>
  <input
    aria-label={label}
    type="range"
    bind:this={rangeInput}
    {min}
    {max}
    {step}
    value={value}
    oninput={(event) => change(Number(event.currentTarget.value))}
  />
</div>
<style>
  .field-hint{display:block;color:#9aa3b8;margin-top:3px;font-size:10px;line-height:1.4}
</style>
