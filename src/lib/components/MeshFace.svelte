<script context="module" lang="ts">
  // The element and its runtime call these (ES2022, Safari 15.4+) without polyfills; an older
  // WKWebView would throw outside the element's error event, so skip straight to the fallback.
  export const MESH_SUPPORTED =
    typeof window !== 'undefined' &&
    'at' in Array.prototype && 'hasOwn' in Object && typeof structuredClone === 'function';
</script>

<script lang="ts">
  import { onDestroy } from 'svelte';
  import 'mesh-avatar-element';
  import type { MeshAvatarElement } from 'mesh-avatar-element';
  import type { CharacterState } from '../stores/character';

  export let state: CharacterState;
  /** Reply emotion from chat-speaking-start / api-push; null when no reply is active. */
  export let emotion: string | null = null;
  /** Set on conversation-barge-in: audio was cut, so the mouth stops before characterState leaves speaking. */
  export let interrupted = false;
  /** Per-sentence truth from the audio player: mouth moves only while a sentence is actually playing. */
  export let mouth = true;
  export let scale = 1;
  export let offsetX = 0;
  export let offsetY = 0;
  /** Character state → mesh expression. */
  export let expressions: Record<string, string>;
  /** PA reply emotion → mesh emotion. */
  export let emotions: Record<string, string>;
  /** Custom `.mavatar` as a data URI, null for the bundled avatar, undefined while still being read. */
  export let customSource: string | null | undefined = undefined;
  export let status: 'loading' | 'ready' | 'failed' = 'loading';
  /** circle: the desktop's face close-up; fill: the whole picture, as large as the box allows, standing on its bottom edge (phone). */
  export let variant: 'circle' | 'fill' = 'circle';

  // Fixed same-origin path, kept even for custom files: <mesh-avatar> without src fetches a remote sample.
  const MESH_SRC = '/avatars/default.mavatar';
  // Baseline framing at scale 1, offset 0. Since 0.3.2 it is the earlier 96px / (-12, -10)
  // crop zoomed to 0.9 and moved down 5px; settings_repository converts older saved values.
  const SIZE = 96 * 0.9;
  const BASE_LEFT = -7.2;
  const BASE_TOP = -0.2;

  let meshEl: MeshAvatarElement | null = null;
  let mouthFrame = 0;
  let loadedCustom: string | null = null;
  // load() cannot go back to `src`, so returning to the bundled avatar recreates the element.
  let builtinGeneration = 0;

  // Map, not an object: emotion comes from pushed data, so keys like "__proto__" must miss.
  $: emotionMap = new Map(Object.entries(emotions));
  $: ready = status === 'ready';
  $: meshEmotion = state === 'idle' || !emotion ? null : (emotionMap.get(emotion) ?? 'neutral');
  $: if (ready) meshEl?.avatar?.setEmotion(meshEmotion, { playMotion: false });
  // `mouth` is true only while TTS is audible, including status speech during thinking.
  $: if (ready && !interrupted && mouth) startMouth(); else stopMouth();
  $: if (meshEl && customSource && customSource !== loadedCustom) loadCustom(meshEl, customSource);
  $: if (!customSource && loadedCustom) {
    loadedCustom = null;
    stopMouth();
    status = 'loading';
    builtinGeneration += 1;
  }

  // Scale around the face center so the default crop stays put.
  $: size = SIZE * scale;
  $: left = BASE_LEFT + offsetX - (SIZE / 2) * (scale - 1);
  $: top = BASE_TOP + offsetY - (SIZE / 2) * (scale - 1);

  // The element renders at min(devicePixelRatio, 2) backing pixels per CSS pixel. On a 100%-scaled
  // display (most Windows PCs: DPR 1) the 1600px picture then drops to ~96 px through mipmaps and
  // the face goes soft. Lay the element out `supersample` times larger and scale it back down, so
  // it always renders at least 2 backing pixels per displayed CSS pixel, as on a Retina Mac.
  const supersample = Math.max(1, 2 / Math.min(typeof window === 'undefined' ? 2 : window.devicePixelRatio || 1, 2));

  let boxW = 0;
  let boxH = 0;
  $: fill = variant === 'fill';
  $: side = fill ? Math.min(boxW, boxH) : size;
  $: elLeft = fill ? (boxW - side) / 2 : left;
  $: elTop = fill ? boxH - side : top;

  function loadCustom(el: MeshAvatarElement, dataUri: string) {
    loadedCustom = dataUri;
    status = 'loading';
    try {
      const binary = atob(dataUri.slice(dataUri.indexOf(',') + 1));
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      // Failures arrive as the element's error event; the promise rejection is redundant.
      el.load(bytes).catch(() => {});
    } catch (e) {
      console.warn('[avatar] custom mesh avatar is unreadable', e);
      status = 'failed';
    }
  }

  // Simulated loudness: two sines (~6 Hz) plus jitter, with occasional near-silent gaps.
  function mouthLevel(ms: number) {
    const t = ms / 1000;
    const wave = 0.5 + 0.3 * Math.sin(t * 2 * Math.PI * 6) + 0.2 * Math.sin(t * 2 * Math.PI * 3.7 + 1);
    const gap = Math.sin(t * 2 * Math.PI * 0.45) > 0.85 ? 0.1 : 1;
    return Math.min(1, Math.max(0, wave * gap + (Math.random() - 0.5) * 0.1));
  }

  function feedMouth(ms: number) {
    meshEl?.setVoiceLevel(mouthLevel(ms));
    mouthFrame = requestAnimationFrame(feedMouth);
  }

  function startMouth() {
    if (mouthFrame) return;
    meshEl?.avatar?.setSpeaking(true);
    mouthFrame = requestAnimationFrame(feedMouth);
  }

  function stopMouth() {
    if (!mouthFrame) return;
    cancelAnimationFrame(mouthFrame);
    mouthFrame = 0;
    meshEl?.setVoiceLevel(0);
    meshEl?.avatar?.setSpeaking(false);
  }

  // Fires again after a WebGL context restore or a custom load with a fresh avatar instance; the
  // element only replays expression and voice level, so push emotion and speaking onto the new one.
  function handleLoad() {
    status = 'ready';
    meshEl?.avatar?.setEmotion(meshEmotion, { playMotion: false });
    if (mouthFrame) meshEl?.avatar?.setSpeaking(true);
  }

  function handleError(e: Event) {
    console.warn('[avatar] mesh avatar unavailable, using fallback skin', (e as CustomEvent).detail?.error);
    stopMouth();
    status = 'failed';
  }

  onDestroy(stopMouth);
</script>

<div class="mesh-face" class:fill aria-hidden="true" bind:clientWidth={boxW} bind:clientHeight={boxH}>
  <!-- Created only once the source is known: a custom load() then replaces the queued `src` load. -->
  {#if customSource !== undefined}
  {#key builtinGeneration}
  <mesh-avatar
    bind:this={meshEl}
    src={MESH_SRC}
    fit="contain"
    expression={expressions[state] ?? 'neutral'}
    style:width="{side * supersample}px"
    style:height="{side * supersample}px"
    style:left="{elLeft}px"
    style:top="{elTop}px"
    style:transform={supersample === 1 ? null : `scale(${1 / supersample})`}
    on:load={handleLoad}
    on:error={handleError}
  ></mesh-avatar>
  {/key}
  {/if}
</div>

<style>
  /* ─── Mesh avatar: face close-up cropped by the circle ─── */
  .mesh-face {
    position: absolute;
    inset: 0;
    overflow: hidden;
    border-radius: 50%;
    /* Older WebKit (macOS 12) ignores the rounded overflow clip for the composited WebGL
       canvas, so the avatar spilled out of the circle; a mask clips composited layers too. */
    -webkit-mask-image: radial-gradient(circle closest-side, #000 99%, transparent 100%);
    mask-image: radial-gradient(circle closest-side, #000 99%, transparent 100%);
    pointer-events: none;
  }
  .mesh-face.fill {
    border-radius: 0;
    -webkit-mask-image: none;
    mask-image: none;
  }
  /* 1600px source at 96px = 0.06 scale; face center (797, 860) → (48, 51.6), placed at (36, 42) — slightly below the 72px padding-box center */
  .mesh-face mesh-avatar {
    position: absolute;
    pointer-events: none;
    transform-origin: 0 0;
  }
</style>
