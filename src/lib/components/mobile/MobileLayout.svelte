<script lang="ts">
  import type { ComponentProps } from 'svelte';
  import { characterState, type CharacterState } from '../../stores/character';
  import { chatStore, statusVisible } from '../../stores/chat';
  import { settingsStore } from '../../stores/settings';
  import { avatarGradient } from '../../avatarColors';
  import MobileAvatar from './MobileAvatar.svelte';
  import DynamicIsland from '../DynamicIsland.svelte';
  import ChatPanel from '../ChatPanel.svelte';
  import StatusPanel from '../StatusPanel.svelte';
  import ReasoningPanel from '../ReasoningPanel.svelte';
  import Icon from '../Icon.svelte';
  import { listen } from '../../ipc';
  import { onDestroy, tick } from 'svelte';

  // Phone portrait (/ui): golden-ratio split — avatar stage on top, a dock
  // below with two swipeable pages: the reply plus the user's transcript, and
  // the tools plus reasoning. The dock slides to the page that matches the
  // turn as it changes phase. The camera button (Android
  // shell only) swaps the dock for a viewfinder: the shell lays its native
  // preview over the box we reserve, and Hermes can grab a frame through PA's
  // /mcp. App.svelte computes every state; this only lays it out.
  let {
    islandMode,
    audioLevel,
    reasoningVisible,
    avatar,
  }: {
    islandMode: ComponentProps<typeof DynamicIsland>['mode'];
    audioLevel: number;
    reasoningVisible: boolean;
    avatar: ComponentProps<typeof MobileAvatar>;
  } = $props();

  const STATES: CharacterState[] = ['idle', 'listening', 'thinking', 'speaking'];
  // One gradient layer per state, cross-faded: gradients themselves can't transition.
  let layers = $derived(STATES.map((s) => ({ state: s, ...avatarGradient(s, avatar.bgColors?.[s]) })));
  // chat.ts keeps 🔧 steps until the reply finishes, so a turn counts as
  // replying as soon as the reply starts, even with steps still listed.
  let replying = $derived($characterState === 'speaking' || $chatStore.streamingContent !== '');
  let sttPlaceholder = $derived($settingsStore.ui_lang === 'en' ? 'What you say shows up here' : '你说的话会显示在这里');
  let toolsPlaceholder = $derived($settingsStore.ui_lang === 'en' ? 'Tools and reasoning show up here' : '工具和思考过程会显示在这里');

  // Page 0: reply + transcript; page 1: tools + reasoning.
  let pager: HTMLDivElement | undefined = $state();
  let page = $state(0);
  let phase = $derived(replying ? 'reply' : $characterState === 'thinking' ? 'think' : null);
  let lastPhase: string | null = null;

  function goTo(i: number) {
    pager?.scrollTo({ left: i * pager.clientWidth, behavior: 'smooth' });
  }

  function onPagerScroll() {
    if (pager) page = Math.round(pager.scrollLeft / pager.clientWidth);
  }

  // Slide only when the phase changes, so a page the user swiped to stays put
  // until the turn moves on.
  $effect(() => {
    if (phase === null) {
      lastPhase = null;
    } else if (phase !== lastPhase) {
      lastPhase = phase;
      goTo(phase === 'think' ? 1 : 0);
    }
  });

  // Injected by android/…/MainActivity.kt; absent in a plain browser.
  type CameraBridge = {
    openCamera(x: number, y: number, w: number, h: number, facing: string): void;
    flipCamera(): void;
    closeCamera(): void;
  };
  const camera = (window as unknown as { PocketAgentCamera?: CameraBridge }).PocketAgentCamera;
  let cameraOn = $state(false);
  let front = false;
  let viewfinder: HTMLDivElement | undefined = $state();
  let resize: ResizeObserver | undefined;

  function placeCamera() {
    if (!camera || !viewfinder) return;
    const r = viewfinder.getBoundingClientRect();
    camera.openCamera(r.left, r.top, r.width, r.height, front ? 'front' : 'back');
  }

  async function toggleCamera() {
    if (!camera) return;
    if (cameraOn) {
      camera.closeCamera();
      return; // the shell answers with pa-camera {on:false}
    }
    cameraOn = true;
    await tick();
    placeCamera();
    if (viewfinder) {
      resize = new ResizeObserver(placeCamera);
      resize.observe(viewfinder);
    }
  }

  function flipCamera() {
    front = !front;
    camera?.flipCamera();
  }

  // The shell closes the camera itself when the app leaves the foreground or
  // the permission is refused.
  function onCameraEvent(e: Event) {
    if ((e as CustomEvent<{ on: boolean }>).detail.on) return;
    cameraOn = false;
    page = 0; // the pager comes back scrolled to the start
    resize?.disconnect();
    resize = undefined;
  }
  if (camera) window.addEventListener('pa-camera', onCameraEvent);

  // Hermes opens / closes the camera through PA (phone_camera_open / _close).
  // Opening goes through toggleCamera like a tap, which marks the camera on
  // before it opens, so a tap and a command never open it twice.
  type CameraCommand = { action: 'open' | 'close'; facing?: 'front' | 'back' };
  function onCameraCommand(cmd: CameraCommand) {
    if (cmd.action === 'close') {
      if (cameraOn) camera?.closeCamera();
      return;
    }
    const wantFront = cmd.facing === 'front';
    if (cameraOn) {
      if (front !== wantFront) flipCamera();
      return;
    }
    front = wantFront;
    void toggleCamera();
  }
  // Registered once per mount, never from an effect.
  const unlistenCamera = camera ? listen<CameraCommand>('phone-camera', (e) => onCameraCommand(e.payload)) : null;

  onDestroy(() => {
    void unlistenCamera?.then((unlisten) => unlisten());
    window.removeEventListener('pa-camera', onCameraEvent);
    resize?.disconnect();
    if (cameraOn) camera?.closeCamera();
  });
</script>

<div class="mobile">
  <section class="stage">
    {#each layers as layer (layer.state)}
      <div
        class="tint"
        class:on={$characterState === layer.state}
        style:background="linear-gradient(180deg, {layer.grad1} 0%, {layer.grad2} 100%)"
        aria-hidden="true"
      ></div>
    {/each}
    <div class="avatar-box"><MobileAvatar {...avatar} /></div>
    <div class="island"><DynamicIsland mode={islandMode} {audioLevel} /></div>
    {#if camera}
      <button
        type="button"
        class="toggle"
        class:on={cameraOn}
        onclick={toggleCamera}
        aria-label={cameraOn ? '关闭相机' : '打开相机'}
        aria-pressed={cameraOn}
      ><Icon name="camera" size={20} color="rgba(232, 232, 240, 0.92)" /></button>
    {/if}
  </section>

  <section class="dock">
    {#if cameraOn}
      <div class="viewfinder" bind:this={viewfinder} aria-label="相机取景"></div>
      <button type="button" class="flip" onclick={flipCamera} aria-label="切换前后摄像头">
        <Icon name="switch-camera" size={18} color="rgba(232, 232, 240, 0.92)" />
        <span>翻转</span>
      </button>
    {:else}
    <div class="pager" bind:this={pager} onscroll={onPagerScroll}>
      <div class="page">
        <div class="panel"><ChatPanel side="right" onCollapse={() => {}} /></div>
        <div class="stt" class:empty={$chatStore.sttText === null} aria-live="polite">
          <Icon name="audio-lines" size={16} color="rgba(160, 168, 255, 0.75)" />
          {#if $chatStore.sttText !== null}
            {#key $chatStore.sttText}<span class="stt-text">{$chatStore.sttText}</span>{/key}
          {:else}
            <span class="stt-text">{sttPlaceholder}</span>
          {/if}
        </div>
      </div>
      <div class="page">
        {#if $statusVisible}
          <StatusPanel maxSteps={3} />
          {#if reasoningVisible}
            <div class="reasoning"><ReasoningPanel visible={reasoningVisible} /></div>
          {/if}
        {:else}
          <div class="tools-empty">{toolsPlaceholder}</div>
        {/if}
      </div>
    </div>
    <div class="dots">
      {#each [0, 1] as i}
        <button
          type="button"
          class="dot"
          class:on={page === i}
          onclick={() => goTo(i)}
          aria-label={i === 0 ? '对话' : '工具和理由'}
          aria-current={page === i}
        ></button>
      {/each}
    </div>
    {/if}
  </section>
</div>

<style>
  .mobile {
    position: fixed;
    inset: 0;
    display: grid;
    grid-template-rows: 61.8fr 38.2fr;
    background: #0b0c18;
  }

  /* ─── Stage: avatar on its state colour ─── */
  .stage {
    position: relative;
    min-height: 0;
    overflow: hidden;
  }
  .tint {
    position: absolute;
    inset: 0;
    opacity: 0;
    transition: opacity 0.6s ease;
  }
  .tint.on { opacity: 1; }
  /* Fade the stage into the dock so the split reads as one surface. */
  .stage::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 72px;
    background: linear-gradient(180deg, transparent, #0b0c18);
    pointer-events: none;
  }
  .avatar-box {
    position: absolute;
    inset: 56px 0 0 0;
  }
  .island {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 14px;
    display: flex;
    justify-content: center;
    z-index: 1;
  }
  .toggle {
    position: absolute;
    top: calc(10px + env(safe-area-inset-top));
    right: 12px;
    z-index: 2;
    width: 44px;
    height: 44px;
    border-radius: 50%;
    border: 1px solid rgba(160, 168, 255, 0.28);
    background: rgba(10, 10, 22, 0.55);
    -webkit-backdrop-filter: blur(12px);
    backdrop-filter: blur(12px);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition: background 0.2s ease, border-color 0.2s ease;
  }
  .toggle.on {
    background: rgba(224, 48, 104, 0.55);
    border-color: rgba(255, 140, 170, 0.6);
  }

  /* ─── Dock: (reply + transcript) | (tools + reasoning), swipeable ─── */
  .dock {
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 4px 12px calc(12px + env(safe-area-inset-bottom));
    box-sizing: border-box;
  }
  /* The native preview covers exactly this box. */
  .viewfinder {
    flex: 1;
    min-height: 0;
    border-radius: 16px;
    background: #000;
  }
  .flip {
    flex-shrink: 0;
    align-self: center;
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 44px;
    padding: 0 20px;
    border-radius: 22px;
    border: 1px solid rgba(160, 168, 255, 0.28);
    background: rgba(20, 20, 38, 0.85);
    color: rgba(232, 232, 240, 0.92);
    font-size: 15px;
    cursor: pointer;
  }
  /* Spans the dock's side padding so pages slide out at the screen edge. */
  .pager {
    flex: 1;
    min-height: 0;
    margin: 0 -12px;
    display: flex;
    overflow-x: auto;
    overflow-y: hidden;
    scroll-snap-type: x mandatory;
    overscroll-behavior-x: contain;
    scrollbar-width: none;
  }
  .pager::-webkit-scrollbar { display: none; }
  .page {
    flex: 0 0 100%;
    min-width: 0;
    scroll-snap-align: start;
    scroll-snap-stop: always;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 0 12px;
    box-sizing: border-box;
  }
  .panel {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .reasoning {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  .tools-empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 15px;
    color: rgba(160, 168, 255, 0.45);
  }
  .dots {
    flex-shrink: 0;
    display: flex;
    justify-content: center;
    margin: -4px 0 -8px;
  }
  .dot {
    width: 28px;
    height: 20px;
    padding: 0;
    border: none;
    background: none;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .dot::before {
    content: '';
    width: 6px;
    height: 6px;
    border-radius: 3px;
    background: rgba(160, 168, 255, 0.3);
    transition: width 0.2s ease, background 0.2s ease;
  }
  .dot.on::before {
    width: 16px;
    background: rgba(160, 168, 255, 0.85);
  }

  .stt {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 44px;
    padding: 8px 16px;
    box-sizing: border-box;
    border-radius: 22px;
    background: rgba(20, 20, 38, 0.85);
    border: 1px solid rgba(160, 168, 255, 0.22);
  }
  .stt-text {
    flex: 1;
    min-width: 0;
    font-size: 15px;
    line-height: 1.4;
    color: rgba(232, 232, 240, 0.92);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    animation: stt-in 0.2s ease-out;
  }
  .stt.empty .stt-text { color: rgba(160, 168, 255, 0.45); animation: none; }
  @keyframes stt-in { from { opacity: 0; transform: translateY(4px); } }

  /* ─── Shared desktop panels, sized for the dock ─── */
  .mobile :global(.chat-panel) {
    width: 100%;
    height: 100%;
    margin: 0 !important;
  }
  .mobile :global(.chat-panel::before),
  .mobile :global(.chat-panel::after),
  .mobile :global(.chat-panel .close-btn) { display: none; }
  .mobile :global(.chat-panel .message-text) { font-size: 16px; line-height: 1.6; }
  .mobile :global(.status-panel .status-content) { max-height: none; padding: 8px 12px; }
  .mobile :global(.status-panel .step) { font-size: 13px; }
  .mobile :global(.reasoning-panel) { flex: 1; min-height: 0; display: flex; flex-direction: column; padding: 8px 12px; }
  .mobile :global(.reasoning-panel .reasoning-content) { flex: 1; min-height: 0; max-height: none; }
  .mobile :global(.reasoning-panel .reasoning-text) { font-size: 13px; line-height: 1.5; }
</style>
