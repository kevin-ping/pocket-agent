<script lang="ts">
  import type { Snippet } from 'svelte';

  let { label, children }: { label: string; children: Snippet } = $props();
</script>

<div class="field">
  <span class="field-label">{label}</span>
  <div class="control">{@render children()}</div>
</div>
