<script lang="ts">
  let {
    label,
    value,
    min,
    max,
    step,
    display,
    change,
  }: {
    label: string;
    value: number;
    min: number;
    max: number;
    step: number;
    display: string;
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
  <span class="field-label">{label}<small>{display}</small></span>
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
