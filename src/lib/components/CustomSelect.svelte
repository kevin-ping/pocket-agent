<script lang="ts">
  interface VoiceGroup {
    group: string;
    voices: { id: string; label: string; }[];
  }

  let {
    value = $bindable(''),
    options = [] as string[] | VoiceGroup[],
    placeholder = '',
  }: {
    value?: string;
    options?: string[] | VoiceGroup[];
    placeholder?: string;
  } = $props();

  let open = $state(false);

  const isGrouped = $derived(options.length > 0 && typeof options[0] === 'object' && 'group' in options[0]);

  function getLabel(val: string): string {
    if (!isGrouped) return val;
    for (const g of options as VoiceGroup[]) {
      const found = g.voices.find(v => v.id === val);
      if (found) return found.label;
    }
    return val;
  }

  function select(val: string) {
    value = val;
    open = false;
  }

  function toggle() {
    open = !open;
  }

  function handleBlur() {
    setTimeout(() => { open = false; }, 150);
  }
</script>

<div class="custom-select" class:open>
  <button class="select-trigger" onclick={toggle} onblur={handleBlur} type="button">
    <span class="select-value">{value ? getLabel(value) : placeholder}</span>
    <span class="select-arrow">&#9660;</span>
  </button>

  {#if open}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="select-dropdown" role="listbox" onmousedown={(e) => e.preventDefault()}>
      {#if isGrouped}
        {#each options as group (group.group)}
          <div class="optgroup-label">{group.group}</div>
          {#each group.voices as v}
            <button
              class="option"
              class:selected={v.id === value}
              onclick={() => select(v.id)}
              type="button"
              role="option"
              aria-selected={v.id === value}
            >{v.label}</button>
          {/each}
        {/each}
      {:else}
        {#each options as opt}
          <button
            class="option"
            class:selected={opt === value}
            onclick={() => select(opt)}
            type="button"
            role="option"
            aria-selected={opt === value}
          >{opt}</button>
        {/each}
      {/if}
    </div>
  {/if}
</div>

<style>
  .custom-select {
    position: relative;
    width: 172px;
    flex-shrink: 0;
  }
  .select-trigger {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    -webkit-appearance: none;
    appearance: none;
    background-color: rgba(255, 255, 255, 0.06);
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 8px;
    padding: 6px 10px;
    color: rgba(232, 232, 240, 0.9);
    font-size: 12px;
    cursor: pointer;
    outline: none;
    font-family: inherit;
    transition: border-color 0.12s;
    gap: 4px;
  }
  .custom-select.open .select-trigger,
  .select-trigger:focus { border-color: rgba(124, 158, 255, 0.5); }
  .select-trigger:hover { border-color: rgba(124, 158, 255, 0.35); }
  .select-value {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1;
    text-align: left;
  }
  .select-arrow {
    font-size: 8px;
    color: rgba(160, 160, 200, 0.5);
    transition: transform 0.15s;
    line-height: 1;
  }
  .custom-select.open .select-arrow { transform: rotate(180deg); }
  .select-dropdown {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    z-index: 200;
    background: #1a1a2e;
    border: 1px solid rgba(124, 158, 255, 0.3);
    border-radius: 8px;
    max-height: 220px;
    overflow-y: auto;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.6);
  }
  .optgroup-label {
    font-size: 11px;
    font-weight: 700;
    color: rgba(124, 158, 255, 0.75);
    padding: 8px 10px 4px;
    letter-spacing: 0.02em;
  }
  .optgroup-label:first-child { padding-top: 6px; }
  .option {
    display: block;
    width: 100%;
    text-align: left;
    padding: 6px 10px;
    border: none;
    background: transparent;
    color: rgba(232, 232, 240, 0.85);
    font-size: 12px;
    cursor: pointer;
    font-family: inherit;
    transition: background 0.08s;
  }
  .option:hover { background: rgba(124, 158, 255, 0.15); }
  .option.selected {
    background: rgba(124, 158, 255, 0.12);
    color: #c8d8ff;
    font-weight: 600;
  }
</style>
