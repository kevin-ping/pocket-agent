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
  /** Custom `.mavatar` as a data URI, or null for the bundled avatar. */
  export let customSource: string | null = null;
  export let status: 'loading' | 'ready' | 'failed' = 'loading';

  // Fixed same-origin path, kept even for custom files: <mesh-avatar> without src fetches a remote sample.
  const MESH_SRC = '/avatars/xingyin.mavatar';
  const SIZE = 96;

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

  // Scale around the face center so the default crop (left -12, top -10) stays put.
  $: size = SIZE * scale;
  $: left = -12 + offsetX - (SIZE / 2) * (scale - 1);
  $: top = -10 + offsetY - (SIZE / 2) * (scale - 1);

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

<div class="mesh-face" aria-hidden="true">
  {#key builtinGeneration}
  <mesh-avatar
    bind:this={meshEl}
    src={MESH_SRC}
    fit="contain"
    expression={expressions[state] ?? 'neutral'}
    style:width="{size}px"
    style:height="{size}px"
    style:left="{left}px"
    style:top="{top}px"
    on:load={handleLoad}
    on:error={handleError}
  ></mesh-avatar>
  {/key}
</div>

<style>
  /* ─── Mesh avatar: face close-up cropped by the circle ─── */
  .mesh-face {
    position: absolute;
    inset: 0;
    overflow: hidden;
    border-radius: 50%;
    pointer-events: none;
  }
  /* 1600px source at 96px = 0.06 scale; face center (797, 860) → (48, 51.6), placed at (36, 42) — slightly below the 72px padding-box center */
  .mesh-face mesh-avatar {
    position: absolute;
    pointer-events: none;
  }
</style>
