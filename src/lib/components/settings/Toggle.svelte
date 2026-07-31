<script lang="ts">
  let {
    label,
    value,
    change,
  }: {
    label: string;
    value: boolean;
    change: () => void;
  } = $props();
</script>

<div class="field">
  <span class="field-label">{label}</span>
  <button
    aria-label={label}
    class="toggle"
    class:on={value}
    onclick={change}
    role="switch"
    aria-checked={value}
  ><span></span></button>
</div>
