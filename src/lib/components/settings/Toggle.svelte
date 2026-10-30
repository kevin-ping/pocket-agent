<script lang="ts">
  let {
    label,
    value,
    hint = '',
    disabled = false,
    change,
  }: {
    label: string;
    value: boolean;
    hint?: string;
    disabled?: boolean;
    change: () => void;
  } = $props();
</script>

<div class="field">
  <span class="field-label">{label}{#if hint}<em class="field-hint">{hint}</em>{/if}</span>
  <button
    aria-label={label}
    class="toggle"
    class:on={value}
    onclick={change}
    {disabled}
    role="switch"
    aria-checked={value}
  ><span></span></button>
</div>
<style>
  .field-hint{display:block;color:#9aa3b8;margin-top:3px;font-size:10px;line-height:1.4}
  .toggle:disabled{opacity:.4;cursor:not-allowed}
</style>
