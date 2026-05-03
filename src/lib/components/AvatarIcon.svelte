<script lang="ts">
  import { characterState, type CharacterState } from '../stores/character';
  import Icon from './Icon.svelte';
  import { createEventDispatcher } from 'svelte';

  export let avatarImage: string | null = null;
  export let mediaSkins: Partial<Record<CharacterState, string>> = {};

  const dispatch = createEventDispatcher<{ expand: void; contextmenu: MouseEvent }>();

  let state: CharacterState;
  $: state = $characterState;

  const STATE_COLORS: Record<CharacterState, { grad1: string; grad2: string; glow: string; ring: string }> = {
    idle:      { grad1: '#6b8cff', grad2: '#3d5af1', glow: 'rgba(107,140,255,0)',    ring: 'rgba(107,140,255,0.35)' },
    listening: { grad1: '#ff6b9d', grad2: '#e03068', glow: 'rgba(255,107,157,0.45)', ring: 'rgba(255,107,157,0.7)'  },
    thinking:  { grad1: '#ffd06b', grad2: '#e08830', glow: 'rgba(255,200,100,0.3)',  ring: 'rgba(255,200,100,0.6)'  },
    speaking:  { grad1: '#6bffd0', grad2: '#30c888', glow: 'rgba(100,255,200,0.35)', ring: 'rgba(100,255,200,0.65)' },
  };

  $: colors = STATE_COLORS[state];
  $: willChange = state !== 'idle' ? 'transform' : 'auto';
  $: effectiveSkin = mediaSkins[state] ? 'media' : (avatarImage ? 'image' : 'css');
  $: mediaSrc = mediaSkins[state] ?? avatarImage ?? '';

  function handleExpand(e: MouseEvent) { e.stopPropagation(); dispatch('expand'); }
  function handleContextMenu(e: MouseEvent) {
    e.preventDefault();
    dispatch('contextmenu', e);
  }
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<div
  class="avatar-wrap state-{state}"
  style:--grad1={colors.grad1}
  style:--grad2={colors.grad2}
  style:--glow={colors.glow}
  style:--ring={colors.ring}
  style:will-change={willChange}

  on:contextmenu={handleContextMenu}
  role="button"
  tabindex="0"
  on:keydown={(e) => e.key === 'Enter' && dispatch('expand')}
  aria-label="Pocket Agent — 状态: {state}"
>
  <!-- SVG angular corner frame (covers the 96×96 avatar area) -->
  <div class="tech-frame" aria-hidden="true">
    <svg class="corner-svg" viewBox="0 0 96 96" xmlns="http://www.w3.org/2000/svg">
      <polyline points="0,18 0,0 18,0"     class="corner-line" />
      <polyline points="78,0 96,0 96,18"   class="corner-line" />
      <polyline points="0,78 0,96 18,96"   class="corner-line" />
      <polyline points="78,96 96,96 96,78" class="corner-line" />
      <line x1="0"  y1="11" x2="6"  y2="11" class="tick-line" />
      <line x1="108" y1="11" x2="102" y2="11" class="tick-line" />
      <line x1="0"  y1="97" x2="6"  y2="97" class="tick-line" />
      <line x1="108" y1="97" x2="102" y2="97" class="tick-line" />
    </svg>
  </div>


  <!--
    avatar-core: 76×76 reference box — ALL rings/arcs are positioned
    relative to THIS box so they share the exact same center as the circle.
  -->
  <div class="avatar-core">
    <!-- Expand chat button (sits on top-right edge of circle) -->
    <!-- svelte-ignore a11y-click-events-have-key-events -->
    <!-- svelte-ignore a11y-no-static-element-interactions -->
    <div
      class="expand-btn"
      on:click={handleExpand}
      on:mousedown|stopPropagation
      role="button"
      tabindex="0"
      aria-label="展开对话"
    >💬</div>
    <!-- Sound wave rings — 3 layers, staggered delay, scale from center -->
    <div class="wave-ring w1" aria-hidden="true"></div>
    <div class="wave-ring w2" aria-hidden="true"></div>
    <div class="wave-ring w3" aria-hidden="true"></div>

    <!-- Thinking: rotating dashed arc -->
    <div class="think-arc" aria-hidden="true"></div>

    <!-- Main circle -->
    <div class="avatar-circle">
      {#if effectiveSkin === 'media' || effectiveSkin === 'image'}
        {#if mediaSrc.startsWith('data:video') || mediaSrc.endsWith('.mp4') || mediaSrc.endsWith('.webm')}
          <!-- svelte-ignore a11y-media-has-caption -->
          <video class="avatar-media" src={mediaSrc} autoplay loop muted playsinline></video>
        {:else}
          <img class="avatar-media" src={mediaSrc} alt="头像" draggable="false" />
        {/if}
      {:else}
        <div class="css-face" style:will-change={willChange}>
          <div class="ear left"></div>
          <div class="ear right"></div>
          <div class="face-inner">
            <div class="eyes">
              <div class="eye left"></div>
              <div class="eye right"></div>
            </div>
            <div class="mouth"></div>
          </div>
        </div>
      {/if}
    </div>
  </div>

  <!-- State label -->
  <div class="state-label" aria-hidden="true">{state.toUpperCase()}</div>
</div>

<style>
  /* ─── Wrapper ─── */
  .avatar-wrap {
    padding-top: 11px;
    width: 108px;
    height: 115px;
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    user-select: none;
    flex-shrink: 0;
    transition: transform 0.15s ease;
  }
  .avatar-wrap:hover  { }
  .avatar-wrap:active { cursor: grabbing; }

  /* ─── Tech corner frame (absolute, covers avatar area) ─── */
  .tech-frame {
    position: absolute;
    /* Center the 96×96 frame over the 76×76 avatar-core.
       avatar-core top = (112 - 76 - 13) / 2 ≈ 11.5px; frame top = 11.5 - 10 = 1.5 → ~2px */
    top: 9px;
    left: 6px;
    width: 96px;
    height: 96px;
    pointer-events: none;
  }
  .corner-svg  { width: 100%; height: 100%; overflow: visible; }
  .corner-line {
    fill: none;
    stroke: var(--ring);
    stroke-width: 1.5;
    stroke-linecap: square;
    transition: stroke 0.3s;
  }
  .tick-line {
    stroke: var(--ring);
    stroke-width: 1;
    opacity: 0.55;
    transition: stroke 0.3s;
  }

  /* ─── Avatar core: 76×76 reference — rings & circle share this center ─── */
  .avatar-core {
    position: relative;
    width: 76px;
    height: 76px;
    flex-shrink: 0;
    /* Allow rings to paint outside the 76×76 box */
    overflow: visible;
  }

  /* ─── Sound wave rings ─── */
  /*
   * Each ring starts at 76×76 (same as avatar-core), top:0 left:0,
   * and uses transform: scale() so it expands from the exact center.
   * transform-origin defaults to "50% 50%" = center of the element.
   */
  .wave-ring {
    display: none;
    position: absolute;
    top: 0;
    left: 0;
    width: 76px;
    height: 76px;
    border-radius: 50%;
    border: 1.5px solid var(--ring);
    pointer-events: none;
    animation: wave-expand 1.8s ease-out infinite;
  }

  .state-listening .wave-ring { display: block; }
  .state-speaking  .wave-ring { display: block; animation-duration: 1.1s; }

  .wave-ring.w1 { animation-delay: 0s;    }
  .wave-ring.w2 { animation-delay: 0.55s; }
  .wave-ring.w3 { animation-delay: 1.10s; }

  @keyframes wave-expand {
    0%   { transform: scale(0.9);  opacity: 0.75; }
    100% { transform: scale(1.18); opacity: 0;    }
  }

  /* ─── Thinking arc ─── */
  /*
   * 90×90 arc, offset -7px so it surrounds the 76px circle perfectly.
   * Spins around its own center = avatar circle center.
   */
  .think-arc {
    display: none;
    position: absolute;
    top: -7px;
    left: -7px;
    width: 90px;
    height: 90px;
    border-radius: 50%;
    border: 1.5px dashed var(--ring);
    border-color: var(--ring) transparent var(--ring) transparent;
    pointer-events: none;
    animation: think-spin 2.4s linear infinite;
  }
  .state-thinking .think-arc { display: block; }

  @keyframes think-spin {
    from { transform: rotate(0deg); }
    to   { transform: rotate(360deg); }
  }

  /* ─── Main circle ─── */
  .avatar-circle {
    position: absolute;
    top: 0;
    left: 0;
    width: 76px;
    height: 76px;
    border-radius: 50%;
    background: radial-gradient(circle at 38% 35%, var(--grad1) 0%, var(--grad2) 100%);
    border: 2px solid var(--ring);
    box-shadow:
      0 0 0 3px var(--glow),
      0 0 18px var(--glow),
      0 4px 20px rgba(0, 0, 0, 0.55),
      inset 0 1px 0 rgba(255, 255, 255, 0.18);
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: border-color 0.3s, box-shadow 0.3s, transform 0.15s ease;
  }
  .avatar-wrap:hover .avatar-circle  { transform: scale(1.06); }
  .avatar-wrap:active .avatar-circle { transform: scale(0.96); }

  /* ─── Media / image skin ─── */
  .avatar-media {
    width: 100%;
    height: 100%;
    object-fit: cover;
    border-radius: 50%;
    display: block;
  }

  /* ─── CSS face ─── */
  .css-face {
    width: 100%;
    height: 100%;
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .ear {
    width: 14px;
    height: 14px;
    background: rgba(255, 255, 255, 0.25);
    border-radius: 50% 50% 0 0;
    position: absolute;
    top: 12px;
    transform-origin: bottom center;
  }
  .ear.left  { left: 6px; }
  .ear.right { right: 6px; }

  .face-inner {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 5px;
    padding-top: 4px;
  }

  .eyes { display: flex; gap: 9px; }

  .eye {
    width: 7px;
    height: 7px;
    background: rgba(10, 10, 30, 0.85);
    border-radius: 50%;
    transform-origin: center;
  }

  .mouth {
    width: 16px;
    height: 5px;
    background: rgba(10, 10, 30, 0.85);
    border-radius: 0 0 8px 8px;
    transform-origin: center;
  }

  /* ─── Expand button (badge on top-right of circle) ─── */
  .expand-btn {
    position: absolute;
    top: -4px;
    right: -4px;
    width: 14px;
    height: 14px;
    font-size: 12px;
    border-radius: 50%;
    background: rgba(14, 14, 26, 0.9);
    border: 1.5px solid var(--ring);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    z-index: 10;
    opacity: 0.7;
    transition: opacity 0.15s ease, transform 0.15s ease, background 0.15s ease;
    pointer-events: auto;
  }
  .expand-btn:hover {
    opacity: 1;
    transform: scale(1.2);
    background: rgba(30, 30, 55, 0.95);
  }
  .expand-btn:active {
    transform: scale(0.9);
  }
  .expand-btn svg {
    stroke: rgba(200, 210, 255, 0.85);
  }

  /* ─── State label ─── */
  .state-label {
    font-family: 'SF Mono', 'Fira Code', 'Cascadia Code', monospace;
    font-size: 8px;
    font-weight: 600;
    letter-spacing: 0.16em;
    color: var(--ring);
    text-align: center;
    margin-top: 5px;
    line-height: 1;
    opacity: 0.8;
    transition: color 0.3s;
    flex-shrink: 0;
  }

  /* ─── State animations ─── */

  /* idle: breathing + blink */
  @keyframes breathe {
    0%, 100% { transform: scaleY(1);    }
    50%       { transform: scaleY(1.03); }
  }
  @keyframes blink {
    0%, 88%, 100% { transform: scaleY(1);    }
    93%           { transform: scaleY(0.08); }
  }
  .state-idle .avatar-circle { animation: breathe 3.2s ease-in-out infinite; }
  .state-idle .eye            { animation: blink 4.5s ease-in-out infinite; }
  .state-idle .eye.right      { animation-delay: 0.18s; }

  /* listening: ear perk */
  @keyframes ear-perk {
    0%, 100% { transform: translateY(0)   rotate(0deg);   }
    50%      { transform: translateY(-4px) rotate(-10deg); }
  }
  .state-listening .ear       { animation: ear-perk 0.9s ease-in-out infinite; }
  .state-listening .ear.right { animation-delay: 0.12s; }

  /* listening: scanline overlay */
  @keyframes scanline-scroll {
    0%   { background-position: 0 0; }
    100% { background-position: 0 20px; }
  }
  .state-listening .avatar-circle::after {
    content: '';
    position: absolute;
    inset: 0;
    border-radius: 50%;
    background: repeating-linear-gradient(
      0deg,
      transparent,
      transparent 2px,
      rgba(255, 107, 157, 0.07) 2px,
      rgba(255, 107, 157, 0.07) 4px
    );
    animation: scanline-scroll 2s linear infinite;
    pointer-events: none;
  }

  /* thinking: head sway */
  @keyframes think-sway {
    0%, 100% { transform: rotate(0deg);  }
    25%      { transform: rotate(-5deg); }
    75%      { transform: rotate(5deg);  }
  }
  .state-thinking .css-face { animation: think-sway 1.6s ease-in-out infinite; }

  /* speaking: mouth open-close */
  @keyframes mouth-talk {
    0%, 100% { transform: scaleY(1);   }
    50%      { transform: scaleY(0.2); }
  }
  .state-speaking .mouth { animation: mouth-talk 0.28s ease-in-out infinite; }
</style>
