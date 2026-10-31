<script lang="ts">
  import { onMount } from 'svelte';
  import { statusItems, refreshComputerStatus } from '../stores/status';

  // Native `title` tooltips never show: the window is non-focusable (focus: false)
  // and macOS only shows them for the active app. Draw our own bubble instead.
  let hoveredId = $state<string | null>(null);
  let pinnedId = $state<string | null>(null);
  let shownId = $derived(hoveredId ?? pinnedId);
  let shown = $derived($statusItems.find((i) => i.id === shownId) ?? null);

  // Grace period so the pointer can travel from the dot into the bubble
  // (to read or scroll a long message) without the bubble vanishing.
  let hideTimer: ReturnType<typeof setTimeout> | null = null;
  function hoverIn(id: string) {
    if (hideTimer) { clearTimeout(hideTimer); hideTimer = null; }
    hoveredId = id;
  }
  function hoverOut() {
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = setTimeout(() => { hoveredId = null; hideTimer = null; }, 250);
  }

  onMount(() => { void refreshComputerStatus(); });
</script>

<div class="status-strip" role="status" aria-live="polite">
  {#if shown}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="tip" role="tooltip" onmouseenter={() => hoverIn(shown.id)} onmouseleave={hoverOut}>
      <span class="tip-label">{shown.label}</span>
      {#if shown.tooltip}<span class="tip-body">{shown.tooltip}</span>{/if}
    </div>
  {/if}
  {#each $statusItems as item (item.id)}
    <button
      type="button"
      class="item"
      class:pill={item.tone === 'active'}
      aria-label={item.tooltip ? `${item.label} — ${item.tooltip}` : item.label}
      onmouseenter={() => hoverIn(item.id)}
      onmouseleave={hoverOut}
      onclick={() => (pinnedId = pinnedId === item.id ? null : item.id)}
    >
      {#if item.tone === 'active'}
        <span class="pulse" aria-hidden="true"></span>
        <span class="pill-label">{item.label}</span>
      {:else}
        {#key item.flash}
          <span class="dot tone-{item.tone}" class:flash={!!item.flash} aria-hidden="true"></span>
        {/key}
      {/if}
    </button>
  {/each}
</div>

<style>
  .status-strip {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    height: 22px;
    padding: 0 8px 2px;
    border-top: 1px solid rgba(255, 255, 255, 0.06);
    flex-shrink: 0;
    user-select: none;
  }

  /* ─── Tooltip bubble (sits above the strip, inside the panel) ─── */
  .tip {
    position: absolute;
    left: 8px;
    right: 8px;
    bottom: calc(100% + 4px);
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 100px; /* panel is 135px tall; scroll the rest */
    overflow-y: auto;
    scrollbar-width: thin;
    scrollbar-color: rgba(160, 168, 255, 0.25) transparent;
    padding: 5px 8px;
    border-radius: 6px;
    background: rgba(24, 24, 40, 0.96);
    border: 1px solid rgba(160, 168, 255, 0.3);
    z-index: 3;
  }
  .tip::-webkit-scrollbar       { width: 3px; }
  .tip::-webkit-scrollbar-thumb { background: rgba(160, 168, 255, 0.25); border-radius: 2px; }
  .tip-label { font-size: 10.5px; color: rgba(232, 232, 240, 0.95); }
  .tip-body  { font-size: 10px; line-height: 1.45; color: rgba(232, 232, 240, 0.65); white-space: pre-wrap; word-break: break-word; }

  /* ─── Items ─── */
  .item {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 5px; /* a bare 6px dot is too small to hover */
    border: 0;
    background: none;
    cursor: default;
    font: inherit;
  }
  .item:hover .dot { transform: scale(1.35); }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
    transition: background 0.3s ease, transform 0.15s ease;
  }
  .dot.tone-ok   { background: rgba(80, 220, 120, 0.85); }
  .dot.tone-warn { background: rgba(255, 200, 80, 0.9); }
  .dot.tone-off  { background: rgba(232, 232, 240, 0.3); }
  /* One-shot red pulse; {#key flash} remounts the dot so it replays per event. */
  /* Strobe: 3 hard on/off blinks, ~0.3s total. */
  .dot.flash { animation: dot-flash 0.1s steps(1, end) 3; }

  .item.pill {
    gap: 5px;
    height: 14px;
    padding: 0 7px 0 5px;
    border-radius: 999px;
    background: rgba(100, 200, 255, 0.12);
    border: 1px solid rgba(100, 200, 255, 0.4);
    animation: pill-in 0.3s cubic-bezier(0.4, 0, 0.2, 1);
  }
  .pill-label {
    font-size: 10px;
    line-height: 1;
    color: rgba(200, 230, 255, 0.92);
    white-space: nowrap;
  }
  .pulse {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: rgba(100, 200, 255, 0.95);
    animation: pulse-breathe 1.4s ease-in-out infinite;
  }

  @keyframes pill-in {
    from { opacity: 0; transform: scaleX(0.4); }
    to   { opacity: 1; transform: scaleX(1); }
  }
  @keyframes dot-flash {
    0%  { background: rgba(255, 60, 60, 1); box-shadow: 0 0 6px 1px rgba(255, 60, 60, 0.9); }
    50% { background: rgba(232, 232, 240, 0.3); box-shadow: none; }
  }
  @keyframes pulse-breathe {
    0%, 100% { opacity: 0.4; }
    50%      { opacity: 1; }
  }
  @media (prefers-reduced-motion: reduce) {
    .item.pill, .pulse { animation: none; }  }
</style>
