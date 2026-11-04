<script lang="ts">
  import { characterState, type CharacterState } from '../../stores/character';
  import MeshFace, { MESH_SUPPORTED } from '../MeshFace.svelte';
  import AvatarIcon from '../AvatarIcon.svelte';

  // The phone's avatar: no circle, frame or rings — the whole picture, as large
  // as the stage allows. Same inputs as AvatarIcon.
  let {
    avatarImage = null,
    mediaSkins = {},
    emotion = null,
    interrupted = false,
    mouthActive = true,
    meshEnabled = true,
    meshExpressions,
    meshEmotions,
    meshCustom = null,
    bgColors = {},
  }: {
    avatarImage?: string | null;
    mediaSkins?: Partial<Record<CharacterState, string>>;
    emotion?: string | null;
    interrupted?: boolean;
    mouthActive?: boolean;
    meshEnabled?: boolean;
    meshExpressions: Record<string, string>;
    meshEmotions: Record<string, string>;
    meshCustom?: string | null;
    bgColors?: Record<string, string>;
  } = $props();

  let meshStatus = $state<'loading' | 'ready' | 'failed'>('loading');
  // A new file or re-enabling gets a fresh attempt even after an earlier failure.
  // Compare values: App passes a fresh props object on every update (mouth, emotion…),
  // and resetting then would drop a ready avatar back to loading and freeze its mouth.
  let meshKey = '';
  $effect.pre(() => {
    const key = `${MESH_SUPPORTED && meshEnabled}|${meshCustom ?? ''}`;
    if (key === meshKey) return;
    meshKey = key;
    meshStatus = MESH_SUPPORTED && meshEnabled ? 'loading' : 'failed';
  });

  let mediaSrc = $derived(mediaSkins[$characterState] ?? avatarImage ?? '');
  let isVideo = $derived(mediaSrc.startsWith('data:video') || mediaSrc.endsWith('.mp4') || mediaSrc.endsWith('.webm'));
</script>

<div class="mobile-avatar" aria-label="Pocket Agent — 状态: {$characterState}">
  {#if meshStatus !== 'failed'}
    <MeshFace variant="fill" state={$characterState} {emotion} {interrupted} mouth={mouthActive}
      expressions={meshExpressions} emotions={meshEmotions} customSource={meshCustom} bind:status={meshStatus} />
  {:else if mediaSrc && isVideo}
    <!-- svelte-ignore a11y_media_has_caption -->
    <video class="media" src={mediaSrc} autoplay loop muted playsinline></video>
  {:else if mediaSrc}
    <img class="media" src={mediaSrc} alt="头像" draggable="false" />
  {:else}
    <!-- No picture at all: the desktop's drawn face, enlarged. -->
    <div class="drawn">
      <AvatarIcon meshEnabled={false} {emotion} {interrupted} {mouthActive} {meshExpressions} {meshEmotions} {bgColors} />
    </div>
  {/if}
</div>

<style>
  .mobile-avatar {
    position: absolute;
    inset: 0;
  }
  .media {
    width: 100%;
    height: 100%;
    object-fit: contain;
    object-position: center bottom;
    display: block;
  }
  .drawn {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    zoom: 2.4;
    pointer-events: none;
  }
</style>
